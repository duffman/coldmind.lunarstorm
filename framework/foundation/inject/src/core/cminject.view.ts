import {
  ServiceLifetime, type InjectedFactory, type InjectedLifecycle, type ExactFactoryOptions, type RegistrySnapshot,
  type ValidationOptions, type InjectedSpec, type ScopeType, type AmbientResolution, type AutoInjectOptions,
  type BindingDescriptor, type BindingHandle, type CacheableBinding, type DiIssue, type DiscoveredClass,
  type DisposalInfo, type HealthStatus, type IContainer, type InstanceHook, type InstanceState, type Interceptor,
  type MultiBindingEntry, type OwnershipRecord, type RegisterOptions, type ReplacementOptions,
  type RuntimeDependencyGraph, type RuntimeDependencyTreeNode, type RuntimeEdge, type RuntimeEpochSnapshot,
  type RuntimeResourceGraph, type ScopeIdentity, type ScopeTreeNode, type ServiceFactory, type ServiceLifecycle,
} from '@coldmind/inject/contracts';
import type { DependencySlot } from '@coldmind/inject/lazy';
import type { ModuleDefinition, ModuleHandle } from '@coldmind/inject/module';
import type { Constructor, InjectionToken } from '@coldmind/inject/token';
import type {
  Binding, StoredRecord, Build, Frame, Resolver, Scope, CompletionLink, ContextHost, ViewTarget, ResolutionOperation,
} from '@coldmind/inject/internal/host';

/** Explicit, compiler-checked public forwarding shared by factory and completion contexts. */
export abstract class ContainerView implements IContainer, Scope {
  load(...modules: ModuleDefinition[]): Promise<ModuleHandle[]> { this.ordinary(); return this.target.load(...modules); }
  snapshot(): RegistrySnapshot { this.ordinary(); return this.target.snapshot(); }
  restore(snapshot: RegistrySnapshot): Promise<void> { this.ordinary(); return this.target.restore(snapshot); }
  /** Actual receiver for every non-contextual operation. */
  constructor(readonly target: ViewTarget, protected readonly registration?: RegistrationCapture) {}
  protected registering<T, D>(work: () => BindingHandle<T, D>): BindingHandle<T, D> {
    this.registration?.active();
    const handle = work(); this.registration?.capture(handle); return handle;
  }
  protected ordinary(): void { if (this.registration) throw new Error('Module registration facade only permits registration and resolution'); }

  /** Internal resolution keeps the contextual synchronous fast path. */
  abstract resolveEither(token: InjectionToken): unknown;
  /** Typed contextual operations owned by the target. */
  protected abstract readonly host: ContextHost;
  /** Active resolver and completion lineage for contextual construction. */
  abstract resolutionContext(): Resolver;
  /** Root-only attachment delegates to the actual scope, so children still reject it. */
  useAmbient(source: AmbientResolution | undefined): void { this.target.useAmbient(source); }
  /** Target-backed buildTimeout. */
  get buildTimeout(): number | null | undefined { return this.target.buildTimeout; }
  set buildTimeout(value: number | null | undefined) { this.target.buildTimeout = value; }
  /** Contextual signal. */
  abstract get signal(): AbortSignal;
  /** Target-backed scopeType. */
  get scopeType(): ScopeType { return this.target.scopeType; }
  /** Target-backed isDisposed. */
  get isDisposed(): boolean { return this.target.isDisposed; }
  /** Target-backed identity. */
  get identity(): ScopeIdentity { return this.target.identity; }
  /** Delegates to the actual target: createScope. */
  createScope(scopeId?: string): IContainer { this.ordinary(); return this.target.createScope(scopeId); }
  /** Delegates to the actual target: createTenantScope. */
  createTenantScope(tenantId?: string): IContainer { this.ordinary(); return this.target.createTenantScope(tenantId); }
  /** Delegates to the actual target: createRequestScope. */
  createRequestScope(requestId?: string): IContainer { this.ordinary(); return this.target.createRequestScope(requestId); }
  /** Delegates to the actual target: singleton. */
  singleton<T, const Slots extends readonly DependencySlot[], Factory extends InjectedFactory<T, Slots>>(token: InjectionToken<T>, lifecycle: InjectedLifecycle<T, Slots, Factory>, ...options: ExactFactoryOptions<Slots, Factory>): BindingHandle<T>;
  singleton<T, D = void>(token: InjectionToken<T>, factory: ServiceFactory<T, D> | (ServiceLifecycle<T, D> & { readonly inject?: never }), options?: RegisterOptions): BindingHandle<T, D>;
  singleton<T>(ctor: Constructor<T>, options?: RegisterOptions): BindingHandle<T>;
  singleton<T, D>(token: InjectionToken<T>, factory?: ServiceFactory<T, D> | ServiceLifecycle<T, D> | InjectedSpec<T> | RegisterOptions, options?: RegisterOptions): BindingHandle<T, D> {
    return this.registering(() => this.target.registerFactory(token, factory, ServiceLifetime.Singleton, 'single', options));
  }
  /** Delegates to the actual target: transient. */
  transient<T, const Slots extends readonly DependencySlot[], Factory extends InjectedFactory<T, Slots>>(token: InjectionToken<T>, lifecycle: InjectedLifecycle<T, Slots, Factory>, ...options: ExactFactoryOptions<Slots, Factory>): BindingHandle<T>;
  transient<T, D = void>(token: InjectionToken<T>, factory: ServiceFactory<T, D> | (ServiceLifecycle<T, D> & { readonly inject?: never }), options?: RegisterOptions): BindingHandle<T, D>;
  transient<T>(ctor: Constructor<T>, options?: RegisterOptions): BindingHandle<T>;
  transient<T, D>(token: InjectionToken<T>, factory?: ServiceFactory<T, D> | ServiceLifecycle<T, D> | InjectedSpec<T> | RegisterOptions, options?: RegisterOptions): BindingHandle<T, D> {
    return this.registering(() => this.target.registerFactory(token, factory, ServiceLifetime.Transient, 'single', options));
  }
  /** Delegates to the actual target: scoped. */
  scoped<T, const Slots extends readonly DependencySlot[], Factory extends InjectedFactory<T, Slots>>(token: InjectionToken<T>, lifecycle: InjectedLifecycle<T, Slots, Factory>, ...options: ExactFactoryOptions<Slots, Factory>): BindingHandle<T>;
  scoped<T, D = void>(token: InjectionToken<T>, factory: ServiceFactory<T, D> | (ServiceLifecycle<T, D> & { readonly inject?: never }), options?: RegisterOptions): BindingHandle<T, D>;
  scoped<T>(ctor: Constructor<T>, options?: RegisterOptions): BindingHandle<T>;
  scoped<T, D>(token: InjectionToken<T>, factory?: ServiceFactory<T, D> | ServiceLifecycle<T, D> | InjectedSpec<T> | RegisterOptions, options?: RegisterOptions): BindingHandle<T, D> {
    return this.registering(() => this.target.registerFactory(token, factory, ServiceLifetime.Scoped, 'single', options));
  }
  /** Delegates to the actual target: tenant. */
  tenant<T, const Slots extends readonly DependencySlot[], Factory extends InjectedFactory<T, Slots>>(token: InjectionToken<T>, lifecycle: InjectedLifecycle<T, Slots, Factory>, ...options: ExactFactoryOptions<Slots, Factory>): BindingHandle<T>;
  tenant<T, D = void>(token: InjectionToken<T>, factory: ServiceFactory<T, D> | (ServiceLifecycle<T, D> & { readonly inject?: never }), options?: RegisterOptions): BindingHandle<T, D>;
  tenant<T>(ctor: Constructor<T>, options?: RegisterOptions): BindingHandle<T>;
  tenant<T, D>(token: InjectionToken<T>, factory?: ServiceFactory<T, D> | ServiceLifecycle<T, D> | InjectedSpec<T> | RegisterOptions, options?: RegisterOptions): BindingHandle<T, D> {
    return this.registering(() => this.target.registerFactory(token, factory, ServiceLifetime.Tenant, 'single', options));
  }
  /** Delegates to the actual target: request. */
  request<T, const Slots extends readonly DependencySlot[], Factory extends InjectedFactory<T, Slots>>(token: InjectionToken<T>, lifecycle: InjectedLifecycle<T, Slots, Factory>, ...options: ExactFactoryOptions<Slots, Factory>): BindingHandle<T>;
  request<T, D = void>(token: InjectionToken<T>, factory: ServiceFactory<T, D> | (ServiceLifecycle<T, D> & { readonly inject?: never }), options?: RegisterOptions): BindingHandle<T, D>;
  request<T>(ctor: Constructor<T>, options?: RegisterOptions): BindingHandle<T>;
  request<T, D>(token: InjectionToken<T>, factory?: ServiceFactory<T, D> | ServiceLifecycle<T, D> | InjectedSpec<T> | RegisterOptions, options?: RegisterOptions): BindingHandle<T, D> {
    return this.registering(() => this.target.registerFactory(token, factory, ServiceLifetime.Request, 'single', options));
  }
  /** Delegates to the actual target: singletonMulti. */
  singletonMulti<T, const Slots extends readonly DependencySlot[], Factory extends InjectedFactory<T, Slots>>(token: InjectionToken<T>, lifecycle: InjectedLifecycle<T, Slots, Factory>, ...options: ExactFactoryOptions<Slots, Factory>): BindingHandle<T>;
  singletonMulti<T, D = void>(token: InjectionToken<T>, factory: ServiceFactory<T, D> | (ServiceLifecycle<T, D> & { readonly inject?: never }), options?: RegisterOptions): BindingHandle<T, D>;
  singletonMulti<T>(ctor: Constructor<T>, options?: RegisterOptions): BindingHandle<T>;
  singletonMulti<T, D>(token: InjectionToken<T>, factory?: ServiceFactory<T, D> | ServiceLifecycle<T, D> | InjectedSpec<T> | RegisterOptions, options?: RegisterOptions): BindingHandle<T, D> {
    return this.registering(() => this.target.registerFactory(token, factory, ServiceLifetime.Singleton, 'multi', options));
  }
  /** Delegates to the actual target: transientMulti. */
  transientMulti<T, const Slots extends readonly DependencySlot[], Factory extends InjectedFactory<T, Slots>>(token: InjectionToken<T>, lifecycle: InjectedLifecycle<T, Slots, Factory>, ...options: ExactFactoryOptions<Slots, Factory>): BindingHandle<T>;
  transientMulti<T, D = void>(token: InjectionToken<T>, factory: ServiceFactory<T, D> | (ServiceLifecycle<T, D> & { readonly inject?: never }), options?: RegisterOptions): BindingHandle<T, D>;
  transientMulti<T>(ctor: Constructor<T>, options?: RegisterOptions): BindingHandle<T>;
  transientMulti<T, D>(token: InjectionToken<T>, factory?: ServiceFactory<T, D> | ServiceLifecycle<T, D> | InjectedSpec<T> | RegisterOptions, options?: RegisterOptions): BindingHandle<T, D> {
    return this.registering(() => this.target.registerFactory(token, factory, ServiceLifetime.Transient, 'multi', options));
  }
  /** Delegates to the actual target: scopedMulti. */
  scopedMulti<T, const Slots extends readonly DependencySlot[], Factory extends InjectedFactory<T, Slots>>(token: InjectionToken<T>, lifecycle: InjectedLifecycle<T, Slots, Factory>, ...options: ExactFactoryOptions<Slots, Factory>): BindingHandle<T>;
  scopedMulti<T, D = void>(token: InjectionToken<T>, factory: ServiceFactory<T, D> | (ServiceLifecycle<T, D> & { readonly inject?: never }), options?: RegisterOptions): BindingHandle<T, D>;
  scopedMulti<T>(ctor: Constructor<T>, options?: RegisterOptions): BindingHandle<T>;
  scopedMulti<T, D>(token: InjectionToken<T>, factory?: ServiceFactory<T, D> | ServiceLifecycle<T, D> | InjectedSpec<T> | RegisterOptions, options?: RegisterOptions): BindingHandle<T, D> {
    return this.registering(() => this.target.registerFactory(token, factory, ServiceLifetime.Scoped, 'multi', options));
  }
  /** Delegates to the actual target: tenantMulti. */
  tenantMulti<T, const Slots extends readonly DependencySlot[], Factory extends InjectedFactory<T, Slots>>(token: InjectionToken<T>, lifecycle: InjectedLifecycle<T, Slots, Factory>, ...options: ExactFactoryOptions<Slots, Factory>): BindingHandle<T>;
  tenantMulti<T, D = void>(token: InjectionToken<T>, factory: ServiceFactory<T, D> | (ServiceLifecycle<T, D> & { readonly inject?: never }), options?: RegisterOptions): BindingHandle<T, D>;
  tenantMulti<T>(ctor: Constructor<T>, options?: RegisterOptions): BindingHandle<T>;
  tenantMulti<T, D>(token: InjectionToken<T>, factory?: ServiceFactory<T, D> | ServiceLifecycle<T, D> | InjectedSpec<T> | RegisterOptions, options?: RegisterOptions): BindingHandle<T, D> {
    return this.registering(() => this.target.registerFactory(token, factory, ServiceLifetime.Tenant, 'multi', options));
  }
  /** Delegates to the actual target: requestMulti. */
  requestMulti<T, const Slots extends readonly DependencySlot[], Factory extends InjectedFactory<T, Slots>>(token: InjectionToken<T>, lifecycle: InjectedLifecycle<T, Slots, Factory>, ...options: ExactFactoryOptions<Slots, Factory>): BindingHandle<T>;
  requestMulti<T, D = void>(token: InjectionToken<T>, factory: ServiceFactory<T, D> | (ServiceLifecycle<T, D> & { readonly inject?: never }), options?: RegisterOptions): BindingHandle<T, D>;
  requestMulti<T>(ctor: Constructor<T>, options?: RegisterOptions): BindingHandle<T>;
  requestMulti<T, D>(token: InjectionToken<T>, factory?: ServiceFactory<T, D> | ServiceLifecycle<T, D> | InjectedSpec<T> | RegisterOptions, options?: RegisterOptions): BindingHandle<T, D> {
    return this.registering(() => this.target.registerFactory(token, factory, ServiceLifetime.Request, 'multi', options));
  }
  /** Delegates to the actual target: registerInstance. */
  registerInstance<T>(token: InjectionToken<T>, instance: T, ownerPolicy?: { ownership: 'owned' | 'external'; reason?: string }): BindingHandle<T> { return this.registering(() => this.target.registerInstance(token, instance, ownerPolicy)); }
  /** Delegates to the actual target: registerClass. */
  registerClass<T>(ctor: Constructor<T>): BindingHandle<T> { return this.registering(() => this.target.registerClass(ctor)); }
  /** Contextual resolve. */
  abstract resolve<T, TData = void>(token: InjectionToken<T>, data?: TData): Promise<T>;
  /** Contextual resolveSync. */
  abstract resolveSync<T, TData = void>(token: InjectionToken<T>, data?: TData): T;
  /** Contextual resolveAll. */
  abstract resolveAll<T, TData = void>(token: InjectionToken<T>, data?: TData): Promise<T[]>;
  /** Target-backed autoInject. */
  get autoInject(): boolean | AutoInjectOptions { return this.target.autoInject; }
  set autoInject(value: boolean | AutoInjectOptions) { this.target.autoInject = value; }
  /** Delegates to the actual target: get. */
  get<T>(token: InjectionToken<T>): T { return this.host.get(this.resolutionContext(), token); }
  /** Delegates to the actual target: tryGet. */
  tryGet<T>(token: InjectionToken<T>): T | undefined { return this.host.tryGet(this.resolutionContext(), token); }
  /** Delegates to the actual target: isRegistered. */
  isRegistered(token: InjectionToken): boolean { return this.target.isRegistered(token); }
  /** Delegates to the actual target: isRegisteredRecursive. */
  isRegisteredRecursive(token: InjectionToken): boolean { return this.target.isRegisteredRecursive(token); }
  /** Delegates to the actual target: has. */
  has(token: InjectionToken): boolean { return this.target.has(token); }
  /** Delegates to the actual target: hasRecursive. */
  hasRecursive(token: InjectionToken): boolean { return this.target.hasRecursive(token); }
  /** Delegates to the actual target: getRegisteredTokens. */
  getRegisteredTokens(): symbol[] { return this.target.getRegisteredTokens(); }
  /** Delegates to the actual target: replace. */
  replace<T, TData = void>(token: InjectionToken<T>, factory: ServiceFactory<T, TData> | ServiceLifecycle<T, TData>, lifetime: ServiceLifetime, options?: ReplacementOptions): Promise<BindingHandle<T, TData>> { this.ordinary(); this.host.assertMutation(this.resolutionContext()); return this.target.replace(token, factory, lifetime, options); }
  /** Delegates to the actual target: replaceBinding. */
  replaceBinding<T, TData = void>(bindingId: string, factory: ServiceFactory<T, TData> | ServiceLifecycle<T, TData>, lifetime?: ServiceLifetime, options?: ReplacementOptions): Promise<BindingHandle<T, TData>> { this.ordinary(); this.host.assertMutation(this.resolutionContext()); return this.target.replaceBinding(bindingId, factory, lifetime, options); }
  /** Delegates to the actual target: unregister. */
  unregister(token: InjectionToken): void | Promise<void> { this.ordinary(); return this.target.unregister(token); }
  /** Delegates to the actual target: unregisterBinding. */
  unregisterBinding(bindingId: string): Promise<boolean> { this.ordinary(); return this.target.unregisterBinding(bindingId); }
  /** Delegates to the actual target: clearInstances. */
  clearInstances(): Promise<void> { this.ordinary(); return this.target.clearInstances(); }
  /** Delegates to the actual target: initializeOwned. */
  initializeOwned(): Promise<void> { return this.target.initializeOwned(); }
  /** Delegates tracked disposal too, preserving the Container public runtime surface. */
  disposeWithTracking(): Promise<DisposalInfo[]> { this.ordinary(); return this.target.disposeWithTracking(); }
  /** Internal runtime-start rollback uses the actual owner receiver, never facade fields. */
  rollbackNewInstances(preexisting: ReadonlySet<string>): Promise<void> { return this.target.rollbackNewInstances(preexisting); }
  /** Delegates to the actual target: dispose. */
  dispose(): Promise<void> { this.ordinary(); return this.target.dispose(); }
  /** Delegates to the actual target: healthCheck. */
  healthCheck(token: InjectionToken): Promise<HealthStatus> { return this.target.healthCheck(token); }
  /** Delegates to the actual target: healthCheckAll. */
  healthCheckAll(): Promise<Map<symbol, HealthStatus>> { return this.target.healthCheckAll(); }
  /** Delegates to the actual target: addInterceptor. */
  addInterceptor(i: Interceptor): void { return this.target.addInterceptor(i); }
  /** Delegates to the actual target: removeInterceptor. */
  removeInterceptor(i: Interceptor): boolean { return this.target.removeInterceptor(i); }
  /** Delegates to the actual target: getScopeIdentity. */
  getScopeIdentity(): ScopeIdentity { return this.target.getScopeIdentity(); }
  /** Delegates to the actual target: getScopeId. */
  getScopeId(): string { return this.target.getScopeId(); }
  /** Delegates to the actual target: getScopeChain. */
  getScopeChain(): ScopeTreeNode[] { return this.target.getScopeChain(); }
  /** Delegates to the actual target: getRegistryEpoch. */
  getRegistryEpoch(): number { return this.target.getRegistryEpoch(); }
  /** Delegates to the actual target: epochSnapshot. */
  epochSnapshot(): RuntimeEpochSnapshot { return this.target.epochSnapshot(); }
  /** Delegates to the actual target: bindingOf. */
  bindingOf(token: InjectionToken): BindingDescriptor | undefined { return this.target.bindingOf(token); }
  /** Delegates to the actual target: multiBindingsOf. */
  multiBindingsOf(token: InjectionToken): MultiBindingEntry[] { return this.target.multiBindingsOf(token); }
  /** Delegates to the actual target: instanceState. */
  instanceState(token: InjectionToken): InstanceState { return this.target.instanceState(token); }
  /** Delegates to the actual target: cachedBindings. */
  cachedBindings(): CacheableBinding[] { return this.target.cachedBindings(); }
  /** Delegates to the actual target: dependencyEdges. */
  dependencyDrift(): DiIssue[] { return this.target.dependencyDrift(); }
  dependencyEdges(): RuntimeEdge[] { return this.target.dependencyEdges(); }
  /** Delegates to the actual target: ownershipRecords. */
  ownershipRecords(): OwnershipRecord[] { return this.target.ownershipRecords(); }
  /** Delegates to the actual target: getDependencyGraph. */
  getDependencyGraph(): { from: string; to: string }[] { return this.target.getDependencyGraph(); }
  /** Delegates to the actual target: getDependencyGraphSnapshot. */
  getDependencyGraphSnapshot(): RuntimeDependencyGraph { return this.target.getDependencyGraphSnapshot(); }
  /** Delegates to the actual target: getDependencyTree. */
  getDependencyTree(token: InjectionToken): RuntimeDependencyTreeNode[] { return this.target.getDependencyTree(token); }
  /** Delegates to the actual target: getResourceGraphSnapshot. */
  getResourceGraphSnapshot(): RuntimeResourceGraph { return this.target.getResourceGraphSnapshot(); }
  /** Delegates to the actual target: graph. */
  graph(): import('@coldmind/inject/graph').DiGraph { return this.target.graph(); }
  /** Delegates to the actual target: issuesObserved. */
  issuesObserved(): DiIssue[] { return this.target.issuesObserved(); }
  /** Delegates to the actual target: validate. */
  validate(options?: ValidationOptions): void { return this.target.validate(options); }
  /** Delegates to the actual target: onInstance. */
  onInstance(key: string, hook: InstanceHook): () => void { return this.target.onInstance(key, hook); }
  /** Delegates to the actual target: discover. */
  discover(key: string): DiscoveredClass[] { return this.target.discover(key); }
  /** Applies members without losing the active completion lineage. */
  injectMembers<T extends object>(instance: T): T { return this.host.injectMembers(instance, this); }
  /** Waits for contextual member injection. */
  injectMembersAsync<T extends object>(instance: T): Promise<T> { return this.host.injectMembersAsync(instance, this); }
  /** Constructs an unregistered class with contextual dependencies. */
  instantiate<T>(ctor: Constructor<T>, ...args: unknown[]): T { return this.host.instantiate(ctor, args, this); }
  /** Waits for contextual construction. */
  instantiateAsync<T>(ctor: Constructor<T>, ...args: unknown[]): Promise<T> { return this.host.instantiateAsync(ctor, args, this); }
}

/**
 * One build: its frame in the resolution path, the context its factory resolves in, and the container the factory sees.
 * `resolve`, `resolveSync` and `resolveAll` carry the build's resolution while it runs; once `done` is set, a retained
 * reference resolves from a fresh root context. Every other member forwards to the container. It is `instanceof Container`
 * but not `===` it: compare `identity.id`.
 */
export class ResolutionView extends ContainerView implements Frame, Resolver, Build {
  /** True once the build has finished. */
  declare done: boolean;
  /** The container that holds the build. */
  declare readonly target: ViewTarget;
  /** The binding being built. */
  declare readonly binding: Binding;
  /** The record of the build. */
  declare readonly record: StoredRecord;
  /** The frame of the build that asked for this one. */
  declare parent: Frame | undefined;
  /** The build this one's waits count against: this materialization itself. */
  declare readonly build: Build | undefined;
  /** As a `Build`: the builds this one awaits right now. Created on the first await. */
  declare waitsOn: Map<Build, number> | undefined;
  /** Staged replacements visible to this build. */
  declare readonly staged: ReadonlyMap<symbol, Binding> | undefined;
  /** Whether interceptors are skipped. */
  declare readonly suppressInterceptors: boolean;
  /** Completion lineage inherited from the caller, independent of the binding frame. */
  declare readonly completion: CompletionLink | undefined;
  declare readonly operation: ResolutionOperation | undefined;
  /**
   * Creates one materialization's explicit frame and build node. The typed host is shared by its owning container.
   */
  constructor(target: ViewTarget, binding: Binding, record: StoredRecord, parent: Frame | undefined, build: Build | undefined | null,
    staged: ReadonlyMap<symbol, Binding> | undefined, suppressInterceptors: boolean, protected readonly host: ContextHost, completion?: CompletionLink, operation?: ResolutionOperation) {
    super(target); this.done = false; this.binding = binding; this.record = record; this.parent = parent;
    this.build = build === null ? this : build; this.waitsOn = undefined; this.staged = staged; this.suppressInterceptors = suppressInterceptors; this.completion = completion;
    this.operation = operation;
  }
  /** This build is the innermost frame of the resolutions its factory starts. */
  get frame(): Frame { return this; }
  /** Build-lifetime signal independent of resolver expiry. */
  get signal(): AbortSignal { return this.record.signal; }
  /** Uses the active binding frame or a fresh frame with any still-valid explicit outer completion. */
  resolutionContext(): Resolver { return this.done ? this.host.freshContext(this.completion) : this; }
  /** Resolves within this build while it runs, from a fresh root context after. */
  resolve<T, D = void>(token: InjectionToken<T>, data?: D): Promise<T> {
    return this.host.resolve(this.resolutionContext(), token, data, this.binding.keyOf(token));
  }
  /** Synchronous `resolve`. Throws `LifecycleError` when any step is asynchronous. */
  resolveSync<T, D = void>(token: InjectionToken<T>, data?: D): T {
    return this.host.resolveSync(this.resolutionContext(), token, data, this.binding.keyOf(token));
  }
  /** Resolves a single token within this build while it runs: its value when synchronous, otherwise a promise of it. */
  resolveEither(token: InjectionToken): unknown {
    return this.host.resolveEither(this.resolutionContext(), token, this.binding.keyOf(token));
  }
  /** Resolves every multi entry within this build while it runs, from a fresh root context after. */
  resolveAll<T, D = void>(token: InjectionToken<T>, data?: D): Promise<T[]> {
    return this.host.resolveAll(this.resolutionContext(), token, data);
  }
}
/** Hook-local context without a synthetic binding, record, proxy or ambient state. */
export class CompletionView extends ContainerView {
  /** Creates a facade sharing the existing target-backed container forwarding boundary. */
  constructor(target: ViewTarget, private readonly link: CompletionLink, protected readonly host: ContextHost, private readonly signalSource: () => AbortSignal) { super(target); }
  /** Only valid hook links participate in reentry or resolution. */
  get completion(): CompletionLink | undefined { return this.host.currentCompletion(this.link); }
  /** Stable originating build or manual-scope lifetime signal. */
  get signal(): AbortSignal { return this.signalSource(); }
  /** Retains active explicit context and falls back normally after a hook expires. */
  resolutionContext(): Resolver {
    const completion = this.host.currentCompletion(this.link);
    const base = completion?.context?.() ?? this.host.context();
    return completion ? { ...base, completion } : base;
  }
  /** Resolves a service in the active hook's resolution and completion contexts. */
  resolve<T, D = void>(token: InjectionToken<T>, data?: D): Promise<T> { return this.host.resolve(this.resolutionContext(), token, data); }
  /** Synchronous contextual resolution. */
  resolveSync<T, D = void>(token: InjectionToken<T>, data?: D): T { return this.host.resolveSync(this.resolutionContext(), token, data); }
  /** Internal sync-or-async contextual slot resolution. */
  resolveEither(token: InjectionToken): unknown { return this.host.resolveEither(this.resolutionContext(), token); }
  /** Resolves every matching multi binding in the active context. */
  resolveAll<T, D = void>(token: InjectionToken<T>, data?: D): Promise<T[]> { return this.host.resolveAll(this.resolutionContext(), token, data); }

}



/** Per-load registration capture; a retained facade becomes invalid after callback settlement. */
export interface RegistrationCapture {
  active(): void;
  capture<T, D>(handle: BindingHandle<T, D>): void;
}
/** Explicit transaction context survives await without changing the target's ambient state. */
export class RegistrationView extends ContainerView {
  constructor(target: ViewTarget, protected readonly host: ContextHost, capture: RegistrationCapture) { super(target, capture); }
  resolutionContext(): Resolver { this.registration!.active(); return this.host.context(); }
  resolveEither(token: InjectionToken): unknown { return this.host.resolveEither(this.resolutionContext(), token); }
  resolve<T, D = void>(token: InjectionToken<T>, data?: D): Promise<T> { return this.host.resolve(this.resolutionContext(), token, data); }
  resolveSync<T, D = void>(token: InjectionToken<T>, data?: D): T { return this.host.resolveSync(this.resolutionContext(), token, data); }
  resolveAll<T, D = void>(token: InjectionToken<T>, data?: D): Promise<T[]> { return this.host.resolveAll(this.resolutionContext(), token, data); }
  get signal(): AbortSignal { return this.target.signal; }
}
