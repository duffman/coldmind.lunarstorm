import { ModuleRegistry, type ModuleOwnedBinding, type ModuleDefinition, type ModuleHandle } from '@coldmind/inject/module';
import { RegistrationView, ContainerView, ResolutionView, CompletionView } from '@coldmind/inject/internal/view';
import type { DependencySlot } from '@coldmind/inject/lazy';
import { Observation, exposeIssues } from '@coldmind/inject/internal/observe';
import { Mutation } from '@coldmind/inject/internal/mutation';
import { Resolution, rootResolver } from '@coldmind/inject/internal/resolution';
export type { AmbientResolution } from '@coldmind/inject/contracts';
import { Construction, currentCompletion } from '@coldmind/inject/internal/construction';
import {
  StoredRecord, TypedBinding, Handle, NO_INSTANCES, hasMultiBinding, type Binding, createBinding, checkTimeout,
  isConstructor, type RegistrationSet,
} from '@coldmind/inject/internal/binding';
import type {
  BindingOwner, RecordOwner, Resolver, ResolutionOperation, CompletionLink, Scope, HookSlot, ContextHost, MutationHost, ObserveHost,
} from '@coldmind/inject/internal/host';
import { now, chain as then, promised, synchronous, settle, syncValue, errorOf } from '@coldmind/inject/internal/async';
import {
  ScopeType, ServiceLifetime, type AmbientResolution, type AutoInjectOptions, type BindingDescriptor,
  type BindingHandle, type CacheableBinding, type DiIssue, type DiscoveredClass, type DisposalInfo, type HealthStatus,
  type IContainer, type InstanceHook, type InstanceState, type Interceptor, type MultiBindingEntry,
  type OwnershipRecord, type RegisterOptions, type ReplacementOptions, type ResolutionPathStep,
  type RuntimeDependencyGraph, type RuntimeDependencyTreeNode, type RuntimeEdge, type RuntimeEpochSnapshot,
  type RuntimeResourceGraph, type ScopeIdentity, type ScopeTreeNode, type ServiceFactory, type ServiceLifecycle,
  type InjectedSpec, type InjectedFactory, type InjectedLifecycle, type ExactFactoryOptions, type RegistrySnapshot,
  type ValidationOptions,
} from '@coldmind/inject/contracts';
import {
  type Constructor, DelayedConstructor, type InjectionToken, nextScopeId, internToken, lookupToken, pinTokenKey,
  releaseTokenKey, retainTokenKey, tokenName,
} from '@coldmind/inject/token';
import {
  DiError, DiGraphError, LifecycleError, MutationConflictError, NotRegisteredError, ScopeMissingError,
} from '@coldmind/inject/errors';
import { DiGraph } from '@coldmind/inject/graph';

/** Message of the `LifecycleError` thrown when a disposed or disposing container is used. */
const DISPOSED_MESSAGE = 'Cannot perform operation on a disposed container';
/** Core-only mapping from record lifetime capabilities to hierarchy owners. */
const recordScopes = new WeakMap<RecordOwner, Container>();
/** Internal hierarchy routing for opaque definition-owner capabilities. */
const ownerScopes = new WeakMap<BindingOwner, Container>();
/** Manual construction uses a stable slot-only facade, never the mutable container instance. */
const scopeOwners = new WeakMap<Scope, Container>();
/** The core alone translates scope capabilities into its private hierarchy. */
function hostOf(scope: Scope): Container {
  if (scope instanceof ContainerView && scope.target instanceof Container) return scope.target;
  const target = scopeOwners.get(scope);
  if (target) return target;
  throw new LifecycleError('Unknown internal resolution scope');
}
interface CapturedRegistry {
  owner: Container;
  target?: InjectionToken;
  scopes: Array<{ scope: Container; definitions: Binding[]; records: Map<Binding, StoredRecord>; epoch: number }>;
  modules: ReturnType<ModuleRegistry['snapshot']> | undefined;
}
const registrySnapshots = new WeakMap<RegistrySnapshot, CapturedRegistry>();
/** Whether the container disposes the instances of a binding: all but those it was told are owned elsewhere. */
const disposes = (binding: Binding): boolean => binding.owner.ownership !== 'external';
/** True when a value implements at least one `IInjectable` lifecycle hook. */
export function isInjectable(value: unknown): value is import('@coldmind/inject/contracts').IInjectable {
  return !!value && typeof value === 'object' && ['onInit', 'onReady', 'onDispose', 'onHealthCheck'].some(key => typeof Reflect.get(value, key) === 'function');
}
/** True when a value has an `onReady` method, run once its scope is initialized. */
export function hasOnReady(value: unknown): value is { onReady(): void | Promise<void> } {
  return !!value && typeof value === 'object' && typeof Reflect.get(value, 'onReady') === 'function';
}
/** Whether a value can carry hook methods: an object or a function. Primitives skip the hook lookup. */
function isObject(value: unknown): value is object {
  return typeof value === 'object' ? value !== null : typeof value === 'function';
}
/** Hierarchical DI registry, scope factory and owner of managed service instances. */
export class Container implements IContainer {
  private moduleRegistry?: ModuleRegistry;
  private readonly moduleLocks = new Set<string>();
  private registryRestoring = false;
  private restoredGeneration = 0;
  /** Rollback restores whole-scope stores, so the matching lock covers the entire hierarchy. */
  private replacementTransaction?: { staged: Map<symbol, Binding>; done: Promise<void>; release(): void; draining: boolean };
  private readonly resolutionOperations = new Set<ResolutionOperation>();
  private get rootContainer(): Container { return this.parent ? this.parent.rootContainer : this; }
  private assertMutable(context?: Resolver): void {
    this.assertActive();
    if (this.rootContainer.registryRestoring) throw new MutationConflictError('Registry restoration is in progress');
    const transaction = this.rootContainer.replacementTransaction;
    if (transaction && this.waitForResolution(context ?? this.rootCtx())) throw new MutationConflictError('Replacement transaction is in progress');
  }
  private waitForResolution(context: Resolver): Promise<void> | undefined {
    const root = this.rootContainer, transaction = root.replacementTransaction;
    if (!transaction || context.staged === transaction.staged) return undefined;
    if (transaction.draining && context.operation && root.resolutionOperations.has(context.operation)) return undefined;
    return transaction.done;
  }
  private beginOperation(): ResolutionOperation {
    const root = this.rootContainer;
    let users = 1, pending: Promise<void> | undefined, release: (() => void) | undefined;
    const operation = {
      // Ordinary synchronous resolution allocates no drain promise.
      get done(): Promise<void> { return pending ??= new Promise<void>(resolve => { release = resolve; }); },
      retain() { users++; },
      finish() { if (--users === 0) { root.resolutionOperations.delete(operation); release?.(); } },
    };
    root.resolutionOperations.add(operation);
    return operation;
  }
  private assertReplacementContext(context: Resolver): void {
    if (context.staged || (context.operation && this.rootContainer.resolutionOperations.has(context.operation))) {
      throw new MutationConflictError('Cannot start a replacement from an active resolution; replace after it completes');
    }
  }
  private async runReplacement<T>(work: (staged: Map<symbol, Binding>) => Promise<T>): Promise<T> {
    this.assertActive();
    this.assertReplacementContext(this.rootCtx());
    const root = this.rootContainer;
    while (root.replacementTransaction) { await root.replacementTransaction.done; this.assertActive(); }
    this.assertMutable();
    root.moduleRegistry?.assertIdle();
    let release!: () => void;
    const done = new Promise<void>(resolve => { release = resolve; });
    const transaction = { staged: new Map<symbol, Binding>(), done, release, draining: true };
    root.replacementTransaction = transaction;
    try {
      // Stop admission first. Existing operations retain their context and can finish nested work.
      await Promise.all([...root.resolutionOperations].map(operation => operation.done));
      this.assertActive();
      transaction.draining = false;
      return await work(transaction.staged);
    } finally {
      if (root.replacementTransaction === transaction) root.replacementTransaction = undefined;
      release();
    }
  }
  private get modules(): ModuleRegistry {
    const root = this.rootContainer;
    return root.moduleRegistry ??= new ModuleRegistry({
      active: () => root.assertActive(),
      cycleError: path => new DiGraphError([{ kind: 'module-cycle', at: Symbol(path[0]), message: `Module cycle: ${path.join(' -> ')}` }]),
      runRegistration: module => root.registerModule(module),
    });
  }
  load(...modules: ModuleDefinition[]): Promise<ModuleHandle[]> { this.assertMutable(); return this.modules.load(...modules); }
  private async registerModule(module: ModuleDefinition): Promise<readonly ModuleOwnedBinding[]> {
    this.assertMutable();
    const handles: ModuleOwnedBinding[] = []; let live = true;
    const facade = new RegistrationView(this, this.contextHost, {
      active: () => { this.assertMutable(); if (!live) throw new LifecycleError('Module registration callback has completed'); },
      capture: handle => {
        const binding = this.order.find(item => item.id === handle.bindingId)!;
        binding.module = module.name; this.moduleLocks.add(binding.id); handles.push(handle);
      },
    });
    try { await module.register(facade); this.assertActive(); return handles; }
    catch (error) {
      const failures: unknown[] = [];
      for (const handle of handles.slice().reverse()) {
        this.moduleLocks.delete(handle.bindingId!);
        if (!this.disposed && !this.disposing) try { await handle.unregister(); } catch (failure) { failures.push(failure); }
      }
      if (failures.length) throw new AggregateError([error, ...failures], `Module rollback failed: ${module.name}`);
      throw error;
    } finally { live = false; for (const handle of handles) this.moduleLocks.delete(handle.bindingId!); }
  }
  snapshot(): RegistrySnapshot {
    this.assertMutable();
    if (this.subtree().some(scope => scope.pending.size || scope.moduleLocks.size)) throw new MutationConflictError('Cannot snapshot pending builds or module registrations');
    const snapshot = Object.freeze({}) as RegistrySnapshot;
    registrySnapshots.set(snapshot, { owner: this, scopes: this.subtree().map(scope => ({ scope, definitions: [...scope.order], records: new Map(scope.records), epoch: scope.epoch })), modules: this.rootContainer.moduleRegistry?.snapshot() });
    return snapshot;
  }
  /** Internal token-local checkpoint used by the testing entry. */
  snapshotBinding(token: InjectionToken): RegistrySnapshot {
    const key = lookupToken(token) ?? internToken(token);
    const owner = this.ancestors().reverse().find(scope => scope.order.some(binding => binding.key === key)) ?? this;
    const snapshot = owner.snapshot(); const captured = registrySnapshots.get(snapshot)!;
    captured.target = token; captured.modules = undefined;
    for (const state of captured.scopes) {
      state.definitions = state.definitions.filter(binding => binding.key === key);
      state.records = new Map([...state.records].filter(([binding]) => binding.key === key));
    }
    return snapshot;
  }
  /** Internal owner routing is limited to a genuine checkpoint from this hierarchy. */
  restoreBinding(snapshot: RegistrySnapshot): Promise<void> {
    const captured = registrySnapshots.get(snapshot);
    if (!captured || captured.owner.rootContainer !== this.rootContainer || captured.target === undefined) throw new MutationConflictError('Foreign binding checkpoint');
    return captured.owner.restore(snapshot);
  }
  async restore(snapshot: RegistrySnapshot): Promise<void> {
    this.assertMutable();
    const captured = registrySnapshots.get(snapshot);
    if (!captured || captured.owner !== this) throw new MutationConflictError('Snapshot belongs to another container');
    this.rootContainer.moduleRegistry?.assertIdle();
    const scopes = this.subtree();
    const targetKey = captured.target === undefined ? undefined : lookupToken(captured.target);
    if (scopes.some(scope => scope.pending.size || scope.moduleLocks.size)) throw new MutationConflictError('Cannot restore pending builds or module registrations');
    // Atomic preflight: irreconstructible owned supplied values cannot be brought back.
    for (const state of captured.scopes) if (!state.scope.disposed && !state.scope.disposing) {
      for (const binding of state.definitions) if (binding.kindTag === 'registerInstance' && binding.owner.ownership === 'owned') {
        const record = state.records.get(binding);
        if (!record || record.state !== 'ready' || record.cleanupStarted || state.scope.records.get(binding) !== record) {
          throw new MutationConflictError(`Cannot restore disposed owned supplied instance: ${tokenName(binding.token)}`);
        }
      }
    }
    this.rootContainer.registryRestoring = true;
    const identities = new Map<string, ModuleOwnedBinding>();
    const cleanupErrors: unknown[] = [];
    try {
      for (const scope of scopes.slice().reverse()) {
        const state = captured.scopes.find(item => item.scope === scope);
        const retired = captured.target === undefined ? [] : scope.order.filter(binding => binding.key === targetKey);
        const keep = new Set(state?.definitions.filter(binding => scope.order.includes(binding)) ?? []);
        for (const binding of [...scope.order].reverse()) if ((captured.target === undefined || binding.key === targetKey) && !keep.has(binding)) try { await scope.mutation.unregisterBinding(binding.id, true); } catch (error) { cleanupErrors.push(error); }
        this.assertActive(); scope.assertActive();
        // Keep only exact live records captured before the test; post-snapshot materializations are disposed.
        for (const record of [...scope.records.values()].reverse()) if ((captured.target === undefined || record.binding.key === targetKey) && state?.records.get(record.binding) !== record) {
          const result = await scope.disposeRecord(record, true); if (result?.error) cleanupErrors.push(result.error); scope.records.delete(record.binding);
          const index = scope.disposalOrder.indexOf(record.binding); if (index >= 0) scope.disposalOrder.splice(index, 1);
        }
        this.assertActive(); scope.assertActive();
        if (!state) continue;
        for (const binding of state.definitions) {
          if (keep.has(binding)) { identities.set(binding.id, { bindingId: binding.id, unregister: () => scope.unregisterBinding(binding.id) }); continue; }
          const restored = createBinding(scope.bindingOwner, binding.token, binding.lifecycle ?? binding.factory, binding.lifetime, binding.kind, binding.kindTag, undefined,
            { deps: [...binding.deps], declared: binding.declared, owner: binding.owner, ctor: binding.ctor, descriptor: binding.descriptor,
              timeoutMs: binding.timeoutMs, generation: binding.generation + ++scope.restoredGeneration, when: binding.when, module: binding.module });
          scope.mutation.install(restored); identities.set(binding.id, restored);
          if (binding.kindTag === 'registerInstance') {
            const original = state.records.get(binding)!;
            const record = scope.resolution.makeRecord(restored, original.data); record.instance = original.instance; record.state = 'ready'; record.produced = true; record.building = false;
            scope.records.set(restored, record); scope.disposalOrder.push(restored);
          }
        }
        if (captured.target !== undefined) for (let index = 0; index < retired.length; index++) {
          const restored = state.definitions[index]; const handle = restored && identities.get(restored.id);
          if (handle) identities.set(retired[index]!.id, handle);
        }
        if (captured.target === undefined) scope.epoch = state.epoch;
        scope.observation.clearRuntime();
      }
      if (captured.modules) this.rootContainer.modules.restore(captured.modules, identities);
      else if (captured.target === undefined) this.rootContainer.moduleRegistry?.reset();
      if (captured.target !== undefined) this.rootContainer.moduleRegistry?.remapBindings(identities);
      if (cleanupErrors.length) throw new AggregateError(cleanupErrors, 'Snapshot cleanup failed');
    } finally { this.rootContainer.registryRestoring = false; }
  }
  /** Opaque ownership identity is shared by children, never by independent roots. */
  private readonly ambientRoot: object;
  /** Only a hierarchy root owns an ambient attachment. */
  private ambientState?: AmbientResolution;
  /** Child lookups always use their current root attachment. */
  private get rootAmbient(): AmbientResolution | undefined { return this.parent ? this.parent.rootAmbient : this.ambientState; }
  /** Attaches/replaces a root's source; explicit factory views remain valid across detachment. */
  useAmbient(source: AmbientResolution | undefined): void {
    this.assertActive();
    if (this.parent) throw new LifecycleError('Ambient resolution can only be attached to a root container');
    this.ambientState = source;
  }
  /** Backing field of `identity`. */
  private identityState?: ScopeIdentity;
  /** Identity fields given at creation. */
  private readonly identityGiven?: Partial<ScopeIdentity>;
  /** Kind of this scope. */
  readonly scopeType: ScopeType;
  /** The parent scope. `undefined` for a root. */
  private parent?: Container;
  /** Live child scopes. A child removes itself when disposed. */
  private readonly children = new Set<Container>();
  /** Registrations made on this container, by token key. */
  private readonly registry = new Map<symbol, RegistrationSet | Binding>();
  /** Bindings in registration order. */
  private readonly order: Binding[] = [];
  /** Instances held by this container, by binding. */
  private readonly records: Map<Binding, StoredRecord>;
  /** Records whose `onReady` has run. `initializeOwned` does not run it again. */
  private readonly readied = new WeakSet<StoredRecord>();
  /** Builds in progress, by binding. Concurrent resolves share them. */
  private readonly pending: Map<Binding, Promise<StoredRecord>>;
  /** This scope's override; null disables an inherited deadline. */
  private timeoutState?: number | null;
  /** Lazy owner-lifetime cancellation source. */
  private lifetimeController?: AbortController;
  /** Cancellation is recorded even when no signal has been requested yet. */
  private lifetimeReason?: DiError;
  /** Bindings in build-completion order. Disposal walks it in reverse. */
  private readonly disposalOrder: Binding[];
  /** Interceptors added on this container. */
  private readonly interceptors: Interceptor[] = [];
  /** Hooks subscribed with `onInstance`. */
  private readonly instanceHooks: HookSlot[] = [];
  /** Registry counter, incremented on every registration change. */
  private epoch = 0;
  /** True once disposal has finished. */
  private disposed = false;
  /** True while disposal runs: resolution and registration are refused. */
  private disposing = false;
  /** The disposal in progress or done. Concurrent `dispose` calls share it. */
  private disposal?: Promise<DisposalInfo[]>;
  /** Actual teardown results still being assembled, available to concurrent tracking callers. */
  private disposalEntries?: DisposalInfo[];
  /** Temporary ownership of result keys until teardown and ancestor handoff finish. */
  private disposalKeys?: Set<symbol>;
  /** Parent collectors consume child results before the child's registrations release their keys. */
  private disposalListeners?: Set<(entry: DisposalInfo) => void>;
  /** Whether a public tracking caller will receive this teardown's result keys. */
  private exposeDisposal = false;
  /** Single-owner resolution/control component; cache fields above alias its exact stores. */
  private readonly resolution: Resolution;
  /** This scope's own `autoInject` setting. `undefined` inherits the parent's. */
  private autoState?: boolean | AutoInjectOptions;

  /** Stable lifetime capabilities for all records owned by this scope. */
  private recordOwnerState?: RecordOwner;
  /** Record helpers observe lifetime without receiving mutable container authority. */
  private get recordOwner(): RecordOwner {
    if (!this.recordOwnerState) {
      const target = this;
      this.recordOwnerState = { get root() { return target.ambientRoot; }, get signal() { return target.signal; }, get lifetimeReason() { return target.lifetimeReason; } };
      recordScopes.set(this.recordOwnerState, this);
    }
    return this.recordOwnerState;
  }
  /** Hierarchy translation remains private to container orchestration. */
  private recordScope(record: StoredRecord): Container {
    const scope = recordScopes.get(record.container);
    if (!scope) throw new LifecycleError('Unknown record owner');
    return scope;
  }

  /** Definition/handle callbacks are bound once; no mutable container escapes into binding helpers. */
  private bindingOwnerState?: BindingOwner;
  /** Stable narrow mutation capabilities used by definitions and retained handles. */
  private get bindingOwner(): BindingOwner {
    if (!this.bindingOwnerState) {
      const target = this;
      this.bindingOwnerState = {
        get identity() { return target.identity; },
        unregisterBinding: id => target.unregisterBinding(id),
        replaceBinding: (id, factory, lifetime, options) => target.replaceBinding(id, factory, lifetime, options),
      };
      ownerScopes.set(this.bindingOwnerState, this);
    }
    return this.bindingOwnerState;
  }
  /** Translates an opaque owner capability only inside container orchestration. */
  private ownerOf(binding: Binding): Container {
    const owner = ownerScopes.get(binding.ownerContainer);
    if (!owner) throw new LifecycleError('Unknown registration owner');
    return owner;
  }

  /** Whether resolving an unregistered class registers it on demand. Inherited from the parent until set here. */
  get autoInject(): boolean | AutoInjectOptions {
    for (let c: Container | undefined = this; c; c = c.parent) if (c.autoState !== undefined) return c.autoState;
    return false;
  }
  set autoInject(value: boolean | AutoInjectOptions) { this.assertActive(); this.autoState = value; }

  /** Async deadline inherited until overridden; null explicitly disables it. */
  get buildTimeout(): number | null | undefined {
    for (let scope: Container | undefined = this; scope; scope = scope.parent) if (scope.timeoutState !== undefined) return scope.timeoutState;
    return undefined;
  }
  set buildTimeout(value: number | null | undefined) { this.assertActive(); checkTimeout(value); this.timeoutState = value; }
  /** Stable scope-lifetime signal, still readable after disposal. */
  get signal(): AbortSignal {
    if (!this.lifetimeController) {
      this.lifetimeController = new AbortController();
      if (this.lifetimeReason) this.lifetimeController.abort(this.lifetimeReason);
    }
    return this.lifetimeController.signal;
  }

  /** Creates a scope under `parent`, or a root when omitted. Use `createScope`, `createTenantScope` or `createRequestScope` for children. */
  constructor(parent?: Container, scopeType: ScopeType = ScopeType.Global, identity?: Partial<ScopeIdentity>) {
    if (parent?.isDisposed) throw new LifecycleError(DISPOSED_MESSAGE);
    this.parent = parent;
    this.scopeType = scopeType;
    this.identityGiven = identity;
    this.ambientRoot = parent?.ambientRoot ?? {};
    const target = this;
    this.resolution = new Resolution({
      get ambient() { return target.rootAmbient; }, get root() { return target.ambientRoot; },
      get identity() { return target.identity; },
      get buildTimeout() { return target.buildTimeout; },
      get recordOwner() { return target.recordOwner; },
      active: () => target.assertActive(),
      waitForResolution: context => target.waitForResolution(context),
      beginOperation: () => target.beginOperation(),
      route: binding => target.route(binding).resolution,
      selectSingle: (key, staged, context) => target.locateSingle(key, staged, context),
      selectMulti: (key, staged) => target.locateMulti(key, staged),
      autoBinding: (token, key) => target.mutation.autoBinding(token, key),
      assertSingle: (key, binding, token) => target.assertNotMulti(key, binding, token),
      hasInterceptors: () => target.hasInterceptors(),
      interceptors: () => target.gatherInterceptors(),
      scopeId: binding => (binding.lifetime === ServiceLifetime.Singleton ? target.ownerOf(binding)
        : binding.lifetime === ServiceLifetime.Tenant ? target.nearestScope(ScopeType.Tenant)
        : binding.lifetime === ServiceLifetime.Request ? target.nearestScope(ScopeType.Request) : target)?.identity.id,
      view: (binding, record, parent, context) => new ResolutionView(target, binding, record, parent, null, context.staged, context.suppressInterceptors, target.contextHost, context.completion, context.operation),
      observe: (from, to) => target.observation.observe(from, to),
      observeResource: (from, to) => target.observation.observeResource(from, to),
      dispose: (record, unreturned) => target.disposeInstance(record, unreturned),
      allBuilds: () => target.ancestors()[0]!.subtree().flatMap(scope => scope.resolution.activeBuilds()),
      exposeIssues,
    });
    this.records = this.resolution.records; this.pending = this.resolution.pending; this.disposalOrder = this.resolution.disposalOrder;
    parent?.children.add(this);
  }
  /** Identity of this scope: id, kind, and tenant or request id. Built on first read: most scopes never show it. */
  get identity(): ScopeIdentity { return this.identityState ??= this.describeScope(); }
  /** Builds the identity: the given fields over the generated id and the tenant and request ids inherited from the parent. */
  private describeScope(): ScopeIdentity {
    const given = this.identityGiven, kind = this.scopeType, parent = this.parent;
    const identity: ScopeIdentity = { kind, id: given?.id ?? (parent ? nextScopeId(kind) : 'root-' + nextScopeId(kind)) };
    if (parent?.identity.tenantId) identity.tenantId = parent.identity.tenantId;
    if (parent?.identity.requestId) identity.requestId = parent.identity.requestId;
    if (given) for (const field in given) Reflect.set(identity, field, Reflect.get(given, field));
    if (kind === ScopeType.Tenant) identity.tenantId = given?.tenantId ?? nextScopeId('tenant');
    if (kind === ScopeType.Request) identity.requestId = given?.requestId ?? nextScopeId('request');
    return identity;
  }
  /** True once `dispose` has run on this container. */
  get isDisposed(): boolean { return this.disposed; }
  /** Throws `LifecycleError` once this container is disposed. */
  private assertActive(): void { if (this.disposed || this.disposing) throw new LifecycleError(DISPOSED_MESSAGE); }
  /** Creates a standalone root container. */
  static createIsolated(): IContainer { return new Container(); }
  /** Creates a generic child scope. Its scoped services are cached here. */
  createScope(scopeId?: string): Container { this.assertActive(); return new Container(this, ScopeType.Generic, scopeId ? { id: scopeId } : undefined); }
  /** Creates a tenant scope. Tenant services are built once per tenant scope. */
  createTenantScope(tenantId?: string): Container {
    this.assertActive(); return new Container(this, ScopeType.Tenant, tenantId ? { tenantId } : undefined);
  }
  /** Creates a request scope. Request services are built once per request scope. */
  createRequestScope(requestId?: string): Container {
    this.assertActive(); return new Container(this, ScopeType.Request, requestId ? { requestId } : undefined);
  }
  /** This container and its ancestors, root first. */
  private ancestors(): Container[] {
    const out: Container[] = []; for (let c: Container | undefined = this; c; c = c.parent) out.push(c);
    return out.reverse();
  }
  /** Every descendant scope, depth first. */
  private allDescendants(): Container[] {
    const result: Container[] = [];
    for (const child of this.children) { result.push(child, ...child.allDescendants()); }
    return result;
  }
  /** This container and every descendant scope. */
  private subtree(): Container[] { return [this, ...this.allDescendants()]; }
  /** Every container related to this one (ancestors and subtree), without duplicates. */
  private view(): Container[] { return [...new Set([...this.ancestors(), ...this.subtree()])]; }
  /** The nearest ancestor-or-self of the given scope type, if any. */
  private nearestScope(type: ScopeType): Container | undefined {
    for (let c: Container | undefined = this; c; c = c.parent) if (c.scopeType === type) return c;
    return undefined;
  }
  /** The container that owns instances of a binding: the root for singletons, the nearest tenant or request scope for those lifetimes, and this container otherwise. */
  private route(binding: Binding): Container {
    switch (binding.lifetime) {
      case ServiceLifetime.Singleton: return this.ownerOf(binding);
      case ServiceLifetime.Scoped:
      case ServiceLifetime.Transient: return this;
      case ServiceLifetime.Tenant: {
        const scope = this.nearestScope(ScopeType.Tenant);
        if (!scope) throw new ScopeMissingError('tenant', tokenName(binding.token));
        return scope;
      }
      case ServiceLifetime.Request: {
        const scope = this.nearestScope(ScopeType.Request);
        if (!scope) throw new ScopeMissingError('request', tokenName(binding.token));
        return scope;
      }
    }
  }
  /** Operational forwarding boundary used only by genuine typed internal facades. */
  registerFactory<T, D>(token: InjectionToken<T>, factory: ServiceFactory<T, D> | ServiceLifecycle<T, D> | InjectedSpec<T> | RegisterOptions | undefined, lifetime: ServiceLifetime, kind: 'single' | 'multi', options?: RegisterOptions): BindingHandle<T, D> {
    return this.mutation.register(token, factory, lifetime, kind, options);
  }
  /** Registers a singleton, built once per owning root container. */
  singleton<T, const Slots extends readonly DependencySlot[], Factory extends InjectedFactory<T, Slots>>(token: InjectionToken<T>, lifecycle: InjectedLifecycle<T, Slots, Factory>, ...options: ExactFactoryOptions<Slots, Factory>): BindingHandle<T>;
  singleton<T, D = void>(token: InjectionToken<T>, factory: ServiceFactory<T, D> | (ServiceLifecycle<T, D> & { readonly inject?: never }), options?: RegisterOptions): BindingHandle<T, D>;
  /** The class is its own token and implementation. */
  singleton<T>(ctor: Constructor<T>, options?: RegisterOptions): BindingHandle<T>;
  singleton<T, D>(token: InjectionToken<T>, factory?: ServiceFactory<T, D> | ServiceLifecycle<T, D> | InjectedSpec<T> | RegisterOptions, options?: RegisterOptions): BindingHandle<T, D> {
    return this.mutation.register(token, factory, ServiceLifetime.Singleton, 'single', options);
  }
  /** Registers a transient, built anew on every resolve. */
  transient<T, const Slots extends readonly DependencySlot[], Factory extends InjectedFactory<T, Slots>>(token: InjectionToken<T>, lifecycle: InjectedLifecycle<T, Slots, Factory>, ...options: ExactFactoryOptions<Slots, Factory>): BindingHandle<T>;
  transient<T, D = void>(token: InjectionToken<T>, factory: ServiceFactory<T, D> | (ServiceLifecycle<T, D> & { readonly inject?: never }), options?: RegisterOptions): BindingHandle<T, D>;
  /** The class is its own token and implementation. */
  transient<T>(ctor: Constructor<T>, options?: RegisterOptions): BindingHandle<T>;
  transient<T, D>(token: InjectionToken<T>, factory?: ServiceFactory<T, D> | ServiceLifecycle<T, D> | InjectedSpec<T> | RegisterOptions, options?: RegisterOptions): BindingHandle<T, D> {
    return this.mutation.register(token, factory, ServiceLifetime.Transient, 'single', options);
  }
  /** Registers a scoped service, built once per container that resolves it. */
  scoped<T, const Slots extends readonly DependencySlot[], Factory extends InjectedFactory<T, Slots>>(token: InjectionToken<T>, lifecycle: InjectedLifecycle<T, Slots, Factory>, ...options: ExactFactoryOptions<Slots, Factory>): BindingHandle<T>;
  scoped<T, D = void>(token: InjectionToken<T>, factory: ServiceFactory<T, D> | (ServiceLifecycle<T, D> & { readonly inject?: never }), options?: RegisterOptions): BindingHandle<T, D>;
  /** The class is its own token and implementation. */
  scoped<T>(ctor: Constructor<T>, options?: RegisterOptions): BindingHandle<T>;
  scoped<T, D>(token: InjectionToken<T>, factory?: ServiceFactory<T, D> | ServiceLifecycle<T, D> | InjectedSpec<T> | RegisterOptions, options?: RegisterOptions): BindingHandle<T, D> {
    return this.mutation.register(token, factory, ServiceLifetime.Scoped, 'single', options);
  }
  /** Registers a tenant-scoped service, built once per tenant scope. */
  tenant<T, const Slots extends readonly DependencySlot[], Factory extends InjectedFactory<T, Slots>>(token: InjectionToken<T>, lifecycle: InjectedLifecycle<T, Slots, Factory>, ...options: ExactFactoryOptions<Slots, Factory>): BindingHandle<T>;
  tenant<T, D = void>(token: InjectionToken<T>, factory: ServiceFactory<T, D> | (ServiceLifecycle<T, D> & { readonly inject?: never }), options?: RegisterOptions): BindingHandle<T, D>;
  /** The class is its own token and implementation. */
  tenant<T>(ctor: Constructor<T>, options?: RegisterOptions): BindingHandle<T>;
  tenant<T, D>(token: InjectionToken<T>, factory?: ServiceFactory<T, D> | ServiceLifecycle<T, D> | InjectedSpec<T> | RegisterOptions, options?: RegisterOptions): BindingHandle<T, D> {
    return this.mutation.register(token, factory, ServiceLifetime.Tenant, 'single', options);
  }
  /** Registers a request-scoped service, built once per request scope. */
  request<T, const Slots extends readonly DependencySlot[], Factory extends InjectedFactory<T, Slots>>(token: InjectionToken<T>, lifecycle: InjectedLifecycle<T, Slots, Factory>, ...options: ExactFactoryOptions<Slots, Factory>): BindingHandle<T>;
  request<T, D = void>(token: InjectionToken<T>, factory: ServiceFactory<T, D> | (ServiceLifecycle<T, D> & { readonly inject?: never }), options?: RegisterOptions): BindingHandle<T, D>;
  /** The class is its own token and implementation. */
  request<T>(ctor: Constructor<T>, options?: RegisterOptions): BindingHandle<T>;
  request<T, D>(token: InjectionToken<T>, factory?: ServiceFactory<T, D> | ServiceLifecycle<T, D> | InjectedSpec<T> | RegisterOptions, options?: RegisterOptions): BindingHandle<T, D> {
    return this.mutation.register(token, factory, ServiceLifetime.Request, 'single', options);
  }
  /** Adds a singleton to the token's multi-registrations (resolved by `resolveAll`). */
  singletonMulti<T, const Slots extends readonly DependencySlot[], Factory extends InjectedFactory<T, Slots>>(token: InjectionToken<T>, lifecycle: InjectedLifecycle<T, Slots, Factory>, ...options: ExactFactoryOptions<Slots, Factory>): BindingHandle<T>;
  singletonMulti<T, D = void>(token: InjectionToken<T>, factory: ServiceFactory<T, D> | (ServiceLifecycle<T, D> & { readonly inject?: never }), options?: RegisterOptions): BindingHandle<T, D>;
  /** The class is its own token and implementation. */
  singletonMulti<T>(ctor: Constructor<T>, options?: RegisterOptions): BindingHandle<T>;
  singletonMulti<T, D>(token: InjectionToken<T>, factory?: ServiceFactory<T, D> | ServiceLifecycle<T, D> | InjectedSpec<T> | RegisterOptions, options?: RegisterOptions): BindingHandle<T, D> {
    return this.mutation.register(token, factory, ServiceLifetime.Singleton, 'multi', options);
  }
  /** Adds a transient to the token's multi-registrations. */
  transientMulti<T, const Slots extends readonly DependencySlot[], Factory extends InjectedFactory<T, Slots>>(token: InjectionToken<T>, lifecycle: InjectedLifecycle<T, Slots, Factory>, ...options: ExactFactoryOptions<Slots, Factory>): BindingHandle<T>;
  transientMulti<T, D = void>(token: InjectionToken<T>, factory: ServiceFactory<T, D> | (ServiceLifecycle<T, D> & { readonly inject?: never }), options?: RegisterOptions): BindingHandle<T, D>;
  /** The class is its own token and implementation. */
  transientMulti<T>(ctor: Constructor<T>, options?: RegisterOptions): BindingHandle<T>;
  transientMulti<T, D>(token: InjectionToken<T>, factory?: ServiceFactory<T, D> | ServiceLifecycle<T, D> | InjectedSpec<T> | RegisterOptions, options?: RegisterOptions): BindingHandle<T, D> {
    return this.mutation.register(token, factory, ServiceLifetime.Transient, 'multi', options);
  }
  /** Adds a scoped service to the token's multi-registrations. */
  scopedMulti<T, const Slots extends readonly DependencySlot[], Factory extends InjectedFactory<T, Slots>>(token: InjectionToken<T>, lifecycle: InjectedLifecycle<T, Slots, Factory>, ...options: ExactFactoryOptions<Slots, Factory>): BindingHandle<T>;
  scopedMulti<T, D = void>(token: InjectionToken<T>, factory: ServiceFactory<T, D> | (ServiceLifecycle<T, D> & { readonly inject?: never }), options?: RegisterOptions): BindingHandle<T, D>;
  /** The class is its own token and implementation. */
  scopedMulti<T>(ctor: Constructor<T>, options?: RegisterOptions): BindingHandle<T>;
  scopedMulti<T, D>(token: InjectionToken<T>, factory?: ServiceFactory<T, D> | ServiceLifecycle<T, D> | InjectedSpec<T> | RegisterOptions, options?: RegisterOptions): BindingHandle<T, D> {
    return this.mutation.register(token, factory, ServiceLifetime.Scoped, 'multi', options);
  }
  /** Adds a tenant-scoped service to the token's multi-registrations. */
  tenantMulti<T, const Slots extends readonly DependencySlot[], Factory extends InjectedFactory<T, Slots>>(token: InjectionToken<T>, lifecycle: InjectedLifecycle<T, Slots, Factory>, ...options: ExactFactoryOptions<Slots, Factory>): BindingHandle<T>;
  tenantMulti<T, D = void>(token: InjectionToken<T>, factory: ServiceFactory<T, D> | (ServiceLifecycle<T, D> & { readonly inject?: never }), options?: RegisterOptions): BindingHandle<T, D>;
  /** The class is its own token and implementation. */
  tenantMulti<T>(ctor: Constructor<T>, options?: RegisterOptions): BindingHandle<T>;
  tenantMulti<T, D>(token: InjectionToken<T>, factory?: ServiceFactory<T, D> | ServiceLifecycle<T, D> | InjectedSpec<T> | RegisterOptions, options?: RegisterOptions): BindingHandle<T, D> {
    return this.mutation.register(token, factory, ServiceLifetime.Tenant, 'multi', options);
  }
  /** Adds a request-scoped service to the token's multi-registrations. */
  requestMulti<T, const Slots extends readonly DependencySlot[], Factory extends InjectedFactory<T, Slots>>(token: InjectionToken<T>, lifecycle: InjectedLifecycle<T, Slots, Factory>, ...options: ExactFactoryOptions<Slots, Factory>): BindingHandle<T>;
  requestMulti<T, D = void>(token: InjectionToken<T>, factory: ServiceFactory<T, D> | (ServiceLifecycle<T, D> & { readonly inject?: never }), options?: RegisterOptions): BindingHandle<T, D>;
  /** The class is its own token and implementation. */
  requestMulti<T>(ctor: Constructor<T>, options?: RegisterOptions): BindingHandle<T>;
  requestMulti<T, D>(token: InjectionToken<T>, factory?: ServiceFactory<T, D> | ServiceLifecycle<T, D> | InjectedSpec<T> | RegisterOptions, options?: RegisterOptions): BindingHandle<T, D> {
    return this.mutation.register(token, factory, ServiceLifetime.Request, 'multi', options);
  }
  /** Read-only observer capabilities have stable scope identity across cross-scope projections. */
  private observeHostState?: ObserveHost;
  private get observeHost(): ObserveHost {
    if (!this.observeHostState) {
      const target = this;
      this.observeHostState = {
        get parent() { return target.parent?.observeHost; }, get identity() { return target.identity; },
        get epoch() { return target.epoch; }, get owner() { return target.bindingOwnerState; },
        get registry() { return target.registry; }, get order() { return target.order; }, get records() { return target.records; },
        get observed() { return target.observation.observed; }, get resourceObserved() { return target.observation.resourceObserved; },
        active: () => target.assertActive(),
        ancestors: () => target.ancestors().map(scope => scope.observeHost), view: () => target.view().map(scope => scope.observeHost),
        descendants: () => target.allDescendants().map(scope => scope.observeHost), ownerOf: binding => target.ownerOf(binding).observeHost,
        selectSingle: key => target.locateSingle(key), selectMulti: key => target.locateMulti(key),
        isRegisteredRecursive: token => target.isRegisteredRecursive(token), parentGraph: () => target.parent?.observation.indexedGraph(),
        validator(binding, validator) {
          if (!binding.ctor || !binding.descriptor) return;
          return validator.check({ ctor: binding.ctor, descriptor: binding.descriptor, container: target });
        },
      };
    }
    return this.observeHostState;
  }
  /** Graph state and weak observation indexes exist only for scopes that use them. */
  private observationState?: Observation;
  private get observation(): Observation { return this.observationState ??= new Observation(this.observeHost); }

  /** Stable mutation authority is assembled once, with no private-field or nominal Container escape. */
  private mutationHostState?: MutationHost;
  private get mutationHost(): MutationHost {
    if (!this.mutationHostState) {
      const target = this;
      this.mutationHostState = {
        get scopeType() { return target.scopeType; }, get owner() { return target.bindingOwner; },
        get registry() { return target.registry; }, get order() { return target.order; },
        get records() { return target.records; }, get pending() { return target.pending; },
        get disposalOrder() { return target.disposalOrder; }, get observed() { return target.observation.observed; },
        get resourceObserved() { return target.observation.resourceObserved; }, get graphIssues() { return target.observation.graphIssues; },
        get graphIssueKeys() { return target.observation.graphIssueKeys; }, get ended() { return target.disposing || target.disposed; },
        get epoch() { return target.epoch; }, set epoch(value) { target.epoch = value; },
        get parent() { return target.parent?.mutationHost; }, get autoInject() { return target.autoInject; },
        get lifetimeReason() { return target.lifetimeReason; },
        active: context => target.assertMutable(context),
        transaction: work => target.runReplacement(work),
        beginOperation: () => target.beginOperation(),
        remapBinding: (id, handle) => target.rootContainer.moduleRegistry?.remapBindings(new Map([[id, handle]])),
        guardMutation: id => { if (target.moduleLocks.has(id)) throw new MutationConflictError(`Module registration owns binding: ${id}`); },
        subtree: () => target.subtree().map(scope => scope.mutationHost), view: () => target.view().map(scope => scope.mutationHost),
        nearestScope: kind => target.nearestScope(kind)?.mutationHost,
        recordScope: record => target.recordScope(record).mutationHost,
        selectMulti: key => target.locateMulti(key), localSingle: key => target.localSingle(key),
        isRegisteredRecursive: token => target.isRegisteredRecursive(token),
        replace: (token, factory, lifetime, options) => target.mutation.replace(token, factory, lifetime, options),
        factoryArgs: (container, slots) => target.construction.dependencies(container, slots),
        classFactory: (ctor, descriptor, params) => target.construction.factory(ctor, descriptor, params),
        handle: (binding, retained) => target.handle(binding, retained),
        entry: (binding, data, context) => target.resolution.entry(binding, data, context),
        makeRecord: (binding, data) => target.resolution.makeRecord(binding, data),
        route: binding => target.route(binding).mutationHost,
        disposeRecord: (record, tracked) => target.disposeRecord(record, tracked),
        indexGraph: () => { void target.observation.indexedGraph(); }, rebuildGraphKey: key => target.observation.rebuildGraphKey(key),
        finishGraphIndex: () => { target.observation.finishIndex(); },
        forgetObserved: () => target.observation.forgetObserved(), restoreGraphIssues: (issues, keys) => target.observation.restoreGraphIssues(issues, keys),
      };
    }
    return this.mutationHostState;
  }
  /** Registration and replacement use the same transaction engine, allocated only when needed. */
  private mutationState?: Mutation;
  private get mutation(): Mutation { return this.mutationState ??= new Mutation(this.mutationHost); }

  /** Slot-only manual scope, bound once and shared by unregistered construction/injection. */
  private constructionScopeState?: Scope;
  /** Manual construction does not hand the helper a mutable Container as a nominal Scope. */
  private get constructionScope(): Scope {
    if (!this.constructionScopeState) {
      const target = this;
      this.constructionScopeState = { get signal() { return target.signal; }, tryGet: token => target.tryGet(token), resolveEither: token => target.resolveEither(token), resolveAll: token => target.resolveAll(token) };
      scopeOwners.set(this.constructionScopeState, this);
    }
    return this.constructionScopeState;
  }
  /** Constructor/completion service and its deliberately mutation-free capability facet. */
  private constructionState?: Construction;
  /** Stable capabilities keep explicit scope context without exposing implementation fields. */
  private get construction(): Construction {
    const target = this;
    return this.constructionState ??= new Construction({
      active: () => target.assertActive(),
      scope(container) {
        if (container instanceof ContainerView) return container;
        if (container instanceof Container) return container.constructionScope;
        throw new LifecycleError('Unknown class factory context');
      },
      states: scope => hostOf(scope).ancestors()[0]!.construction.states,
      hooks: scope => hostOf(scope).ancestors().flatMap(holder => holder.instanceHooks),
      isRegistered: (scope, token) => hostOf(scope).isRegisteredRecursive(token),
      context: scope => hostOf(scope).rootCtx(),
      hookView: (scope, link) => hostOf(scope).completionView(link, () => scope.signal),
    });
  }

  /** Stable typed capabilities shared by both context facades, allocated once per container. */
  private contextHostState?: ContextHost;
  /** Private resolution/construction callbacks never expose the mutable container implementation. */
  private get contextHost(): ContextHost {
    const target = this;
    return this.contextHostState ??= {
      get<T>(context: Resolver, token: InjectionToken<T>): T { return target.requireCached(token, context).instance as T; },
      tryGet<T>(context: Resolver, token: InjectionToken<T>): T | undefined {
        target.assertActive(); return target.lookupCached(lookupToken(token) ?? Symbol(tokenName(token)), context)?.instance as T | undefined;
      },
      assertMutation: context => target.assertReplacementContext(context),
      currentCompletion,
      freshContext: completion => rootResolver(undefined, completion),
      implementation<T>(token: InjectionToken<T>): Constructor<T> {
        const ctor = token instanceof DelayedConstructor ? token.get() : token;
        if (!isConstructor(ctor)) throw new TypeError(`${tokenName(token)} needs a factory`);
        return ctor;
      },
      context: () => target.rootCtx(),
      resolve<T, D>(context: Resolver, token: InjectionToken<T>, data: D | undefined, known?: symbol): Promise<T> { return promised<T>(() => target.resolution.resolveIn(context, token, data, known)); },
      resolveSync<T, D>(context: Resolver, token: InjectionToken<T>, data: D | undefined, known?: symbol): T { return synchronous<T>(target.resolution.resolveIn(context, token, data, known), token); },
      resolveEither: (context, token, known) => then(target.resolution.resolveIn(context, token, undefined, known), record => record.instance),
      resolveAll: (context, token, data) => target.resolution.resolveAllIn(context, token, data),
      injectMembers: (instance, scope) => syncValue(target.construction.fill(instance, scope), `Injecting ${instance.constructor.name}`, 'injectMembersAsync'),
      injectMembersAsync: (instance, scope) => settle(() => target.construction.fill(instance, scope)),
      instantiate: (ctor, args, scope) => syncValue(target.construction.build(ctor, args, scope), `Building ${ctor.name}`, 'instantiateAsync'),
      instantiateAsync: (ctor, args, scope) => settle(() => target.construction.build(ctor, args, scope)),
    };
  }
  /** Supplies a hook facade without a synthetic binding or per-hook operation table. */
  private completionView(link: CompletionLink, signal: () => AbortSignal): CompletionView {
    return new CompletionView(this, link, this.contextHost, signal);
  }
  /** Builds `ctor` without registering it: `args` fill its first constructor parameters, the container resolves the rest. */
  instantiate<T>(ctor: Constructor<T>, ...args: unknown[]): T {
    return syncValue(this.construction.build(ctor, args, this.constructionScope), `Building ${ctor.name}`, 'instantiateAsync');
  }
  /** As `instantiate`, for a class with asynchronous dependencies. */
  instantiateAsync<T>(ctor: Constructor<T>, ...args: unknown[]): Promise<T> { return settle(() => this.construction.build(ctor, args, this.constructionScope)); }
  /** Applies the members, wraps and instance hooks of an instance's class to it, once per instance. */
  injectMembers<T extends object>(instance: T): T {
    return syncValue(this.construction.fill(instance, this.constructionScope), `Injecting ${instance.constructor.name}`, 'injectMembersAsync');
  }
  /** As `injectMembers`, for a class with asynchronous or multi-registered dependencies. */
  injectMembersAsync<T extends object>(instance: T): Promise<T> { return settle(() => this.construction.fill(instance, this.constructionScope)); }

  /** Public handle for a binding, exposing `unregister` and `replace`. A binding that retains nothing is its own handle. */
  private handle<T, D>(binding: TypedBinding<T, D>, retainedInstances: readonly T[] = NO_INSTANCES): BindingHandle<T, D> {
    return retainedInstances.length ? new Handle<T, D>(this.bindingOwner, binding, retainedInstances) : binding;
  }
  /** The single binding registered directly on this container. */
  private localSingle(key: symbol): Binding | undefined { return this.registry.get(key)?.single; }
  /** The nearest single binding for a key. Staged replacements are checked first. */
  private locateSingle(key: symbol, staged?: ReadonlyMap<symbol, Binding>, context?: Resolver): Binding | undefined {
    const candidate = staged?.get(key);
    if (candidate && candidate.kind === 'single') return candidate;
    for (let c: Container | undefined = this; c; c = c.parent) {
      const entry = c.registry.get(key);
      if (!entry?.single) continue;
      if (!context) return entry.single;
      const candidates = 'singles' in entry && entry.singles ? entry.singles : [entry.single];
      const steps: ResolutionPathStep[] = [];
      for (let frame = context.frame; frame; frame = frame.parent) steps.push({ bindingId: frame.binding.id, tokenName: tokenName(frame.binding.token), scopeId: frame.target.identity.id });
      const path = steps.reverse(); const condition = { path, requester: path.at(-1), scope: this.identity };
      const match = candidates.find(binding => binding.when?.(condition)) ?? candidates.find(binding => !binding.when);
      if (!match) throw new NotRegisteredError(entry.single.token, `${tokenName(entry.single.token)} (no matching conditions or fallback)`);
      return match;
    }
    return undefined;
  }
  /** Every multi binding for a key across ancestors, with staged replacements applied. */
  private locateMulti(key: symbol, staged?: ReadonlyMap<symbol, Binding>): Binding[] {
    const entries = this.ancestors().flatMap(scope => scope.registry.get(key)?.multi ?? []);
    const candidate = staged?.get(key);
    if (candidate?.kind === 'multi') {
      return entries.map(b => b.id === candidate.replacesId ? candidate : b);
    }
    return entries;
  }
  /** Interceptors registered on this container and its ancestors. */
  private gatherInterceptors(): Interceptor[] { return this.ancestors().flatMap(container => container.interceptors); }
  /** Runs the instance's `onDispose`, unless it is externally owned. Returns a promise only when the hook is async. */
  private disposeInstance(record: StoredRecord, unreturned = false): unknown {
    if ((!unreturned && record.binding.lifetime === ServiceLifetime.Transient) || !disposes(record.binding)) return undefined;
    if (record.cleanupStarted) return record.cleanupPromise;
    record.cleanupStarted = true;
    const instance = record.instance;
    // The callable-slot check establishes the optional instance lifecycle shape before invocation.
    const out = record.binding.lifecycle?.onDispose ? record.binding.lifecycle.onDispose(instance)
      : isObject(instance) && typeof (instance as { onDispose?: unknown }).onDispose === 'function' ? (instance as { onDispose(): unknown }).onDispose() : undefined;
    if (out instanceof Promise) record.cleanupPromise = out;
    return out;
  }
  /** Whether this container or an ancestor has an interceptor. */
  private hasInterceptors(): boolean {
    for (let c: Container | undefined = this; c; c = c.parent) if (c.interceptors.length) return true;
    return false;
  }
  /** Throws when a multi registration sits between this scope and the single's owner. A multi above the owner is shadowed by the single. */
  private assertNotMulti(key: symbol, binding: Binding, token: InjectionToken): void {
    if (!hasMultiBinding(key)) return;
    for (let c: Container | undefined = this; c; c = c.parent) {
      if (c.registry.get(key)?.multi.length) throw new MutationConflictError(`Token has multi registrations; use resolveAll(): ${tokenName(token)}`);
      if (c.bindingOwnerState === binding.ownerContainer) return;
    }
  }

  /** Registers supplied values eagerly, preserving owned/external teardown policy. */
  registerInstance<T>(token: InjectionToken<T>, instance: T, ownerPolicy?: { ownership: 'owned' | 'external'; reason?: string }): BindingHandle<T> { return this.mutation.registerInstance(token, instance, ownerPolicy); }
  /** Registers decorated/static classes through the same definition preflight/commit authority. */
  registerClass<T>(ctor: Constructor<T>): BindingHandle<T> { return this.mutation.registerClass(ctor); }

  /** Resolves through the owning explicit resolution pipeline. */
  resolve<T, D = void>(token: InjectionToken<T>, data?: D): Promise<T> { return this.resolution.resolve(token, data); }
  /** Active ambient factory frame, or a fresh external root. */
  private rootCtx(): Resolver { return this.resolution.rootCtx(); }
  /** Internal synchronous-or-asynchronous slot path used by constructor facades. */
  resolveEither(token: InjectionToken): unknown { return this.resolution.resolveEither(token); }
  /** Keeps synchronous graphs synchronous; async graphs retain their normal controlled build. */
  resolveSync<T, D = void>(token: InjectionToken<T>, data?: D): T { return this.resolution.resolveSync(token, data); }
  /** Each caller owns its multi batch; entries still share their individual controlled builds. */
  resolveAll<T, D = void>(token: InjectionToken<T>, data?: D): Promise<T[]> { return this.resolution.resolveAll(token, data); }

  /**
   * The built instance this scope sees for a single token. The nearest binding decides: an unbuilt
   * local binding shadows a parent's. A scoped instance this scope built for a parent binding is found
   * in this scope's own records first.
   */
  private lookupCached(key: symbol, context: Resolver = this.rootCtx()): StoredRecord | undefined {
    if (this.waitForResolution(context)) throw new LifecycleError('Replacement transaction is in progress; use resolve() to wait');
    const binding = this.locateSingle(key, context.staged, context);
    if (!binding) return undefined;
    for (let scope: Container | undefined = this; scope; scope = scope.parent) {
      const record = scope.records.get(binding);
      if (record?.state === 'ready') return record;
    }
    return undefined;
  }
  /** Synchronous read of a built singleton. Throws `LifecycleError` when the service is registered but not built, and `NotRegisteredError` when it is not registered. */
  get<T>(token: InjectionToken<T>): T {
    return this.requireCached(token, this.rootCtx()).instance as T;
  }
  private requireCached(token: InjectionToken, context: Resolver): StoredRecord {
    this.assertActive();
    const key = lookupToken(token) ?? Symbol(tokenName(token));
    const value = this.lookupCached(key, context);
    if (!value) {
      if (!this.isRegisteredRecursive(token)) throw new NotRegisteredError(token, tokenName(token));
      throw new LifecycleError(`Service not initialized: ${tokenName(token)}`);
    }
    return value;
  }
  /** Synchronous read of a built singleton, or `undefined` when it is not built. */
  tryGet<T>(token: InjectionToken<T>): T | undefined {
    // The token establishes T; an absent cache record remains undefined.
    this.assertActive(); return this.lookupCached(lookupToken(token) ?? Symbol(tokenName(token)))?.instance as T | undefined;
  }
  /** Whether this container itself has a registration for the token. */
  isRegistered(token: InjectionToken): boolean { this.assertActive(); const reg = this.registry.get(lookupToken(token) ?? Symbol(tokenName(token))); return !!reg && (!!reg.single || reg.multi.length > 0); }
  /** Whether this container or an ancestor has a registration for the token. */
  isRegisteredRecursive(token: InjectionToken): boolean {
    this.assertActive(); return this.isRegistered(token) || (this.parent?.isRegisteredRecursive(token) ?? false);
  }
  /** Whether this container has a registration for the token, or a ready instance of it. */
  has(token: InjectionToken): boolean {
    this.assertActive(); const key = lookupToken(token) ?? Symbol(tokenName(token));
    return this.isRegistered(token) || [...this.records.values()].some(r => r.key === key && r.state === 'ready');
  }
  /** Whether `has` holds here or in an ancestor. */
  hasRecursive(token: InjectionToken): boolean { this.assertActive(); return this.has(token) || (this.parent?.hasRecursive(token) ?? false); }
  /** Keys of the tokens registered directly on this container. */
  getRegisteredTokens(): symbol[] { this.assertActive(); return this.order.map(b => pinTokenKey(b.key)); }
  /** Adds an interceptor that wraps every resolution made through this container and its descendants. */
  addInterceptor(interceptor: Interceptor): void { this.assertActive(); this.interceptors.push(interceptor); }
  /** Removes an interceptor. Returns `false` when it was not registered here. */
  removeInterceptor(interceptor: Interceptor): boolean {
    this.assertActive(); const index = this.interceptors.indexOf(interceptor);
    if (index < 0) return false; this.interceptors.splice(index, 1); return true;
  }
  /** Subscribes to instances whose methods carry a decorator `key`. Returns an unsubscribe function. */
  onInstance(key: string, hook: InstanceHook): () => void {
    this.assertActive(); const item = { key, hook }; this.instanceHooks.push(item);
    return () => { const index = this.instanceHooks.indexOf(item); if (index !== -1) this.instanceHooks.splice(index, 1); };
  }

  /** A copy of this scope's identity. */
  getScopeIdentity(): ScopeIdentity { return { ...this.identity }; }
  /** This scope's id. */
  getScopeId(): string { return this.identity.id; }
  /** Counter that changes whenever this container's registrations change. */
  getRegistryEpoch(): number { return this.epoch; }
  /** The registry epoch with the scope that owns it and the time it was taken. */
  epochSnapshot(): RuntimeEpochSnapshot { return { epoch: this.epoch, ownerScopeId: this.identity.id, createdAt: now() }; }

  /** Root-first class discovery through read-only metadata. */
  discover(key: string): DiscoveredClass[] { return this.observation.discover(key); }
  /** Runtime/declared diagnostic projection, without exposing private ownership state. */
  getScopeChain(): ScopeTreeNode[] { return this.observation.getScopeChain(); }
  /** Runtime/declared diagnostic projection, without exposing private ownership state. */
  bindingOf(token: InjectionToken): BindingDescriptor | undefined { return this.observation.bindingOf(token); }
  /** Runtime/declared diagnostic projection, without exposing private ownership state. */
  multiBindingsOf(token: InjectionToken): MultiBindingEntry[] { return this.observation.multiBindingsOf(token); }
  /** Runtime/declared diagnostic projection, without exposing private ownership state. */
  instanceState(token: InjectionToken): InstanceState { return this.observation.instanceState(token); }
  /** Runtime/declared diagnostic projection, without exposing private ownership state. */
  cachedBindings(): CacheableBinding[] { return this.observation.cachedBindings(); }
  /** Runtime/declared diagnostic projection, without exposing private ownership state. */
  ownershipRecords(): OwnershipRecord[] { return this.observation.ownershipRecords(); }
  /** Runtime/declared diagnostic projection, without exposing private ownership state. */
  dependencyDrift(): DiIssue[] { return this.observation.drift(); }
  dependencyEdges(): RuntimeEdge[] { return this.observation.dependencyEdges(); }
  /** Runtime/declared diagnostic projection, without exposing private ownership state. */
  getDependencyGraphSnapshot(): RuntimeDependencyGraph { return this.observation.getDependencyGraphSnapshot(); }
  /** Runtime/declared diagnostic projection, without exposing private ownership state. */
  getDependencyGraph(): { from: string; to: string }[] { return this.observation.getDependencyGraph(); }
  /** Runtime/declared diagnostic projection, without exposing private ownership state. */
  getDependencyTree(token: InjectionToken): RuntimeDependencyTreeNode[] { return this.observation.getDependencyTree(token); }
  /** Runtime/declared diagnostic projection, without exposing private ownership state. */
  getResourceGraphSnapshot(): RuntimeResourceGraph { return this.observation.getResourceGraphSnapshot(); }
  /** Runtime/declared diagnostic projection, without exposing private ownership state. */
  graph(): DiGraph { return this.observation.graph(); }
  /** Runtime/declared diagnostic projection, without exposing private ownership state. */
  issuesObserved(): DiIssue[] { return this.observation.issuesObserved(); }
  /** Checks declarations and existing validators, preserving actual issue-key exposure. */
  validate(options?: ValidationOptions): void { return this.observation.validate(options); }

  /** Initializes registrations actually owned by this scope, then emits onReady in order, once per instance. */
  async initializeOwned(): Promise<void> {
    return this.resolution.run({ suppressInterceptors: true }, context => this.initializeIn(context));
  }
  private async initializeIn(ctx: Resolver): Promise<void> {
    this.assertActive();
    const chain = this.ancestors();
    const singles = new Map<symbol, Binding>();
    for (const container of chain) for (const binding of container.order) if (binding.kind === 'single') singles.set(binding.key, binding);
    const owns = (binding: Binding): boolean =>
      (binding.lifetime === ServiceLifetime.Singleton && binding.ownerContainer === this.bindingOwnerState)
      || (binding.lifetime === ServiceLifetime.Tenant && this.scopeType === ScopeType.Tenant)
      || (binding.lifetime === ServiceLifetime.Request && this.scopeType === ScopeType.Request)
      || (binding.lifetime === ServiceLifetime.Scoped && this.scopeType === ScopeType.Generic);
    const orderedSingles = chain.flatMap(c => c.order).filter(b => b.kind === 'single' && singles.get(b.key) === b && owns(b));
    const orderedMulti = chain.flatMap(c => c.order).filter(b => b.kind === 'multi' && owns(b));
    const instances: StoredRecord[] = [];
    for (const binding of [...orderedSingles, ...orderedMulti]) instances.push(await this.resolution.entry(binding, undefined, ctx));
    for (const item of instances) {
      if (this.readied.has(item)) continue;
      const instance = item.instance;
      if (hasOnReady(instance)) await instance.onReady();
      this.readied.add(item);
    }
  }

  /**
   * Disposes one live record. A tracked disposal reports every outcome with its timing; an untracked one reports failures
   * only. Awaits only an asynchronous hook.
   */
  private async disposeRecord(record: StoredRecord, track: boolean): Promise<DisposalInfo | undefined> {
    if (!disposes(record.binding)) return undefined;
    const started = track ? now() : 0;
    record.state = 'disposing';
    record.abort(record.container.lifetimeReason ?? new DiError(`Service disposed: ${tokenName(record.binding.token)}`, 'DI_BUILD_CANCELLED'));
    try {
      const out = this.disposeInstance(record);
      if (out instanceof Promise) await out;
      record.state = 'disposed';
      return track ? { token: record.key, status: 'disposed', durationMs: now() - started } : undefined;
    } catch (error) {
      record.state = 'failed';
      return { token: record.key, status: 'failed', error: errorOf(error), ...(track ? { durationMs: now() - started } : {}) };
    }
  }
  /** Disposes this container and its descendants, returning each service's outcome. Failures do not stop the rest. */
  disposeWithTracking(): Promise<DisposalInfo[]> {
    if (this.disposed) return Promise.resolve([]);
    this.exposeDisposal = true;
    for (const entry of this.disposalEntries ?? []) pinTokenKey(entry.token);
    return this.disposeTree(true);
  }
  /** Collects only real teardown results, retaining their keys across child-to-parent handoff. */
  private appendDisposal(entry: DisposalInfo): void {
    if (!this.disposalKeys?.has(entry.token) && retainTokenKey(entry.token)) (this.disposalKeys ??= new Set()).add(entry.token);
    if (this.exposeDisposal) pinTokenKey(entry.token);
    this.disposalEntries!.push(entry);
    for (const listener of this.disposalListeners ?? []) listener(entry);
  }
  /**
   * Starts this container's disposal, or joins the one in progress: concurrent callers share it, so each onDispose runs once.
   * An untracked disposal reports failures only, without timings. A tracked caller that joins one gets the same.
   */
  private disposeTree(track: boolean): Promise<DisposalInfo[]> {
    if (this.disposed) return Promise.resolve([]);
    if (this.disposal) return this.disposal;
    // Publish identity before abort handlers run: a handler may synchronously join disposal.
    let resolve!: (entries: DisposalInfo[]) => void;
    let reject!: (error: unknown) => void;
    const pending = this.disposal = new Promise<DisposalInfo[]>((yes, no) => { resolve = yes; reject = no; });
    void this.runDisposal(track).then(resolve, reject);
    return pending;
  }
  /** Disposes children, then this container's instances in reverse build order, then clears state. */
  private async runDisposal(track: boolean): Promise<DisposalInfo[]> {
    const scopes = this.subtree();
    for (const scope of scopes) {
      scope.disposing = true; scope.ambientState = undefined;
      if (!scope.parent) scope.moduleRegistry?.invalidate();
      scope.lifetimeReason ??= new DiError(`Scope disposed: ${scope.identity.id}`, 'DI_BUILD_CANCELLED');
    }
    if (!this.parent) this.replacementTransaction?.release();
    for (const scope of scopes) {
      scope.resolution.cancel();
      scope.lifetimeController?.abort(scope.lifetimeReason);
    }
    const result: DisposalInfo[] = [];
    this.disposalEntries = result;
    try {
      if (this.children.size) for (const child of [...this.children]) {
        const collect = (entry: DisposalInfo): void => { this.appendDisposal(entry); };
        (child.disposalListeners ??= new Set()).add(collect);
        for (const entry of child.disposalEntries ?? []) this.appendDisposal(entry);
        try { await child.disposeTree(track); }
        finally { child.disposalListeners?.delete(collect); }
      }
      // Cancellation has removed active materializations. Drain only already-terminal shared outcomes;
      // uncooperative factories and late owned cleanup never delay scope disposal.
      if (this.pending.size) await Promise.allSettled([...this.pending.values()]);
      // Singles and multis share one reverse-construction pass. Records never listed in the order go first, so last here.
      const listed = new Set<StoredRecord>();
      for (let i = this.disposalOrder.length - 1; i >= 0; i--) {
        const record = this.records.get(this.disposalOrder[i]!);
        if (!record || listed.has(record)) continue;
        listed.add(record);
        if (record.state !== 'ready') continue;
        const entry = await this.disposeRecord(record, track);
        if (entry) this.appendDisposal(entry);
      }
      if (this.records.size > listed.size) {
        const unordered = [...this.records.values()].filter(r => r.state === 'ready' && !listed.has(r)).reverse();
        for (const record of unordered) {
          const entry = await this.disposeRecord(record, track);
          if (entry) this.appendDisposal(entry);
        }
      }
      this.disposed = true;
      this.disposing = false;
      this.observationState?.disposeGraph();
      this.parent?.children.delete(this);
      for (const binding of this.order) binding.releaseTokens();
      this.children.clear(); this.registry.clear(); this.order.length = 0;
      this.records.clear(); this.pending.clear(); this.disposalOrder.length = 0;
      this.observationState?.clearRuntime();
      this.instanceHooks.length = 0; this.interceptors.length = 0;
      return result;
    } finally {
      for (const key of this.disposalKeys ?? []) releaseTokenKey(key);
      this.disposalKeys = undefined; this.disposalEntries = undefined;
      this.disposalListeners?.clear(); this.disposalListeners = undefined;
    }
  }
  /** Disposes this container and its descendants. Throws `AggregateError` when any service's disposal failed. */
  async dispose(): Promise<void> {
    const results = await this.disposeTree(false);
    const errors = results.filter(r => r.status === 'failed').map(r => r.error!);
    if (errors.length) throw new AggregateError(errors, `Disposal failed for ${errors.length} service(s)`);
  }
  /**
   * Disposes the instances this container built, in reverse build order, and drops them, so the next resolve builds anew.
   * Waits for builds in progress first. Instances supplied through `registerInstance` and externally owned ones are kept.
   * Throws `AggregateError` when a disposal hook failed; every instance is dropped regardless.
   */
  async clearInstances(): Promise<void> {
    this.assertMutable();
    const operation = this.beginOperation();
    try { await this.clearInstancesIn(); } finally { operation.finish(); }
  }
  private async clearInstancesIn(): Promise<void> {
    if (this.pending.size) await Promise.allSettled([...this.pending.values()]);
    const kept = (record: StoredRecord): boolean => record.binding.kindTag === 'registerInstance' || record.binding.owner.ownership === 'external';
    const failures: Error[] = [];
    const clear = async (record: StoredRecord): Promise<void> => {
      if (record.state !== 'ready') return;
      const entry = await this.disposeRecord(record, false);
      if (entry?.status === 'failed') failures.push(entry.error!);
    };
    const listed = new Set<StoredRecord>();
    for (let i = this.disposalOrder.length - 1; i >= 0; i--) {
      const record = this.records.get(this.disposalOrder[i]!);
      if (!record || listed.has(record)) continue;
      listed.add(record);
      if (!kept(record)) await clear(record);
    }
    for (const record of [...this.records.values()].reverse()) if (!listed.has(record) && !kept(record)) await clear(record);
    for (const [id, record] of this.records) {
      if (!kept(record) && record.state !== 'creating' && record.state !== 'initializing') this.records.delete(id);
    }
    this.disposalOrder.splice(0, this.disposalOrder.length, ...this.disposalOrder.filter(id => this.records.has(id)));
    this.observationState?.clearRuntime();
    if (failures.length) throw new AggregateError(failures, `Disposal failed for ${failures.length} service(s)`);
  }
  /** Health check for one instance, using its `onHealthCheck` when present. */
  private async checkInstance(record: StoredRecord): Promise<HealthStatus> {
    try {
      if (record.binding.lifecycle?.onHealthCheck) return await record.binding.lifecycle.onHealthCheck(record.instance);
      // Health lookup probes this optional callable slot before invoking it below.
      const instance = record.instance as { onHealthCheck?: () => HealthStatus | Promise<HealthStatus> } | undefined;
      if (instance && typeof instance.onHealthCheck === 'function') return await instance.onHealthCheck();
      return { healthy: true };
    } catch (error) { return { healthy: false, error: errorOf(error).message }; }
  }
  /** Health of the token's built instance. Reports unhealthy when it is not built. */
  async healthCheck(token: InjectionToken): Promise<HealthStatus> {
    this.assertActive();
    const key = lookupToken(token) ?? Symbol(tokenName(token));
    const entries = this.locateMulti(key);
    const binding = this.locateSingle(key);
    if (!entries.length && !binding) return { healthy: false, error: 'Service not registered' };
    const selected = entries.length ? entries : [binding!];
    const statuses: HealthStatus[] = [];
    for (const entry of selected) {
      const scopes = entry.lifetime === ServiceLifetime.Singleton ? [this.ownerOf(entry)] : this.subtree();
      const found = scopes.flatMap(c => [...c.records.values()].filter(r => r.binding === entry && r.state === 'ready'));
      if (!found.length) statuses.push({ healthy: false, error: 'Service not initialized' });
      else for (const r of found) statuses.push(await this.checkInstance(r));
    }
    const errors = statuses.filter(s => !s.healthy);
    return errors.length ? { healthy: false, error: errors.map(s => s.error ?? 'Unhealthy').join('; ') } : { healthy: true };
  }
  /** Health of every built instance owned by this container. */
  async healthCheckAll(): Promise<Map<symbol, HealthStatus>> {
    this.assertActive(); const result = new Map<symbol, HealthStatus>();
    for (const binding of this.order) result.set(pinTokenKey(binding.key), await this.healthCheck(binding.token));
    return result;
  }
  /** Transactional replacement, with the existing resolution engine controlling candidates. */
  replaceBinding<T, D = void>(bindingId: string, factory: ServiceFactory<T, D> | ServiceLifecycle<T, D>, lifetime?: ServiceLifetime, options?: ReplacementOptions): Promise<BindingHandle<T, D>> { return this.mutation.replaceBinding(bindingId, factory, lifetime, options); }
  /** Replaces the selected single definition without bypassing normal ownership/rollback policy. */
  replace<T, D = void>(token: InjectionToken<T>, factory: ServiceFactory<T, D> | ServiceLifecycle<T, D>, lifetime: ServiceLifetime, options?: ReplacementOptions): Promise<BindingHandle<T, D>> { return this.mutation.replace(token, factory, lifetime, options); }
  /** @internal Runtime startup rollback shares the central mutation/disposal authority. */
  rollbackNewInstances(preexisting: ReadonlySet<string>): Promise<void> { return this.mutation.rollbackNewInstances(preexisting); }
  /** Removes only registrations owned by this scope. */
  unregister(token: InjectionToken): Promise<void> { return this.mutation.unregister(token); }
  /** Removes exactly one selected binding generation. */
  unregisterBinding(bindingId: string): Promise<boolean> { return this.mutation.unregisterBinding(bindingId); }

}


/** The sole one-way instanceof attachment; view modules never import Container at runtime. */
Object.setPrototypeOf(ContainerView.prototype, Container.prototype);
