import { unwrapDependencySlot, isDeferredToken } from '@coldmind/inject/lazy';
import { ServiceLifetime, type IContainer, type ResourceLifetime, type RegisterOptions, type InstanceRecord, type BindingDescriptor, type BindingHandle, type ClassDescriptor, type DependencyEdge, type ReplacementOptions, type ServiceFactory, type ServiceLifecycle } from '@coldmind/inject/contracts';
import { type Constructor, type InjectionToken, internToken, lookupToken, nextResourceInstanceId, nextSequence, releaseTokenKey, retainTokenKey, tokenName } from '@coldmind/inject/token';
import type { BindingOwner, RecordOwner } from '@coldmind/inject/internal/host';
import type { DiError } from '@coldmind/inject/errors';
import { stamp, buildSignal } from '@coldmind/inject/internal/async';

/** A factory with its data type erased, as stored on a binding. */
export type AnyFactory = (c: IContainer, data?: unknown) => unknown;
/** Operational registry identity erases only the public replacement payload contract; no internal caller invokes it. */
export type Binding = Omit<TypedBinding, 'replace' | 'single' | 'multi'> & {
  /** Registry links carry the same erased operational identity, not a public handle's data type. */
  readonly single: Binding | undefined;
  /** Multi lookup never needs the public replacement signature. */
  readonly multi: readonly Binding[];
};
/** A registration as the container stores it: the public descriptor plus what resolution needs. */
export interface BindingShape extends BindingDescriptor {
  /** Registry key of the token. */
  key: symbol;
  /** Node identity in the dependency graph: the token key for a single, a fresh symbol per multi entry. */
  graphId: symbol;
  /** Id of the binding this one replaces, while a replacement is staged. */
  replacesId?: string;
  /** Builds the instance. Receives the resolution-bound container and the resolve data. */
  factory: AnyFactory;
  /** Lifecycle hooks given with the factory, if any. */
  lifecycle?: ServiceLifecycle<unknown, unknown>;
  /** Declared dependency edges. */
  deps: readonly DependencyEdge[];
  /** Whether the dependencies were declared. Undeclared bindings are not checked for missing dependencies. */
  declared: boolean;
  /** The class, for a class registration. */
  ctor?: Constructor;
  /** The class descriptor, for a class registration. */
  descriptor?: ClassDescriptor;
  /** The container the binding was registered on. */
  ownerContainer: BindingOwner;
  /** Per-registration deadline; undefined inherits the resolving owner. */
  readonly timeoutMs?: number | null;
}
/** Owner policy of every container-built binding. Shared: it is never mutated. */
const OWNED: Binding['owner'] = Object.freeze({ ownership: 'owned' as const });
/** Dependencies of a binding that declares none. Shared: dependency lists are never mutated. */
const NO_DEPS: readonly DependencyEdge[] = Object.freeze([]);
/** `kindTag` of each lifetime, for single and multi registrations. */
export const KIND_TAGS: Record<ServiceLifetime, readonly [string, string]> = {
  [ServiceLifetime.Singleton]: ['singleton', 'singletonMulti'], [ServiceLifetime.Transient]: ['transient', 'transientMulti'],
  [ServiceLifetime.Scoped]: ['scoped', 'scopedMulti'], [ServiceLifetime.Tenant]: ['tenant', 'tenantMulti'], [ServiceLifetime.Request]: ['request', 'requestMulti'],
};
/** The empty multi list every registry group starts with. Frozen: a group gets its own list on its first multi. */
const NO_BINDINGS: readonly Binding[] = Object.freeze([]);
/** Live multi-definition counts across all containers. Other keys skip the multi conflict check. */
const multiKeys = new Map<symbol, number>();
/** Whether the token has any live multi definition across roots. */
export const hasMultiBinding = (key: symbol): boolean => multiKeys.has(key);
/** No retained instances. Shared by every handle that retains none. */
export const NO_INSTANCES: readonly never[] = Object.freeze([]);
/** Handle returned by a registration: `unregister` and `replace` act on the binding it was created for. */
export class Handle<T, D> implements BindingHandle<T, D> {
  /** Creates the handle of `binding` on `container`. */
  constructor(private readonly container: BindingOwner, private readonly binding: TypedBinding<T, D>, readonly retainedInstances: readonly T[]) {}
  /** Id of the binding. */
  get bindingId(): string { return this.binding.id; }
  /** The registered token. */
  get token(): InjectionToken<T> { return this.binding.token; }
  /** Whether this is a single or a multi registration. */
  get kind(): 'single' | 'multi' { return this.binding.kind; }
  /** Generation of the binding. */
  get generation(): number { return this.binding.generation; }
  /** Removes this binding. Resolves `false` if it was already removed. */
  unregister(): Promise<boolean> { return this.container.unregisterBinding(this.bindingId); }
  /** Replaces this binding's factory and returns a handle to the replacement. */
  replace(factory: ServiceFactory<T, D> | ServiceLifecycle<T, D>, lifetime: ServiceLifetime = this.binding.lifetime, options?: ReplacementOptions): Promise<BindingHandle<T, D>> {
    return this.container.replaceBinding<T, D>(this.bindingId, factory, lifetime, options);
  }
}

/** A registration as the container stores it. Its id is formatted on first read, so registering never builds a string. */
export class TypedBinding<T = unknown, D = unknown> implements BindingShape, BindingHandle<T, D> {
  /** Fields only some bindings have. Created on first write, so a plain registration allocates one object. */
  private declare extra: BindingExtra | undefined;
  /** String keys retained by the definition, deduplicated across token and declared dependencies. */
  private declare retainedKeys: Set<symbol> | undefined;
  /** The last dynamically cached string key when it is not already retained by the definition. */
  private declare dynamicKey: symbol | undefined;
  /** False after unregister, rollback or disposal releases this definition. */
  private retentionLive = true;
  /** Time of registration. */
  declare readonly registeredAt: number;
  /** The container the binding was registered on. */
  declare readonly ownerContainer: BindingOwner;
  /** The registered token. */
  declare readonly token: InjectionToken<T>;
  /** Registry key of the token. */
  declare readonly key: symbol;
  /** Lifetime of the instances. */
  declare readonly lifetime: ServiceLifetime;
  /** Whether this is a single or a multi registration. */
  declare readonly kind: 'single' | 'multi';
  /** Builds the instance. Receives the resolution-bound container and the resolve data. */
  declare readonly factory: AnyFactory;
  /** Creates a binding owned by `ownerContainer`. `extra` holds every field that differs from its default. */
  constructor(ownerContainer: BindingOwner, token: InjectionToken<T>, key: symbol, lifetime: ServiceLifetime, kind: 'single' | 'multi',
    factory: AnyFactory, extra: BindingExtra | undefined) {
    this.extra = extra; this.registeredAt = stamp();
    this.ownerContainer = ownerContainer; this.token = token; this.key = key; this.lifetime = lifetime; this.kind = kind; this.factory = factory;
    if (retainTokenKey(key)) this.retainedKeys = new Set([key]);
    for (const dep of extra?.deps ?? NO_DEPS) {
      if (!this.retainedKeys?.has(dep.key) && retainTokenKey(dep.key)) (this.retainedKeys ??= new Set()).add(dep.key);
    }
    if (kind === 'multi') multiKeys.set(key, (multiKeys.get(key) ?? 0) + 1);
  }
  /**
   * The key of a token this binding's factory resolves. Its last string key is retained while cached, then released
   * when another token replaces it or the binding is removed.
   */
  keyOf(token: InjectionToken): symbol | undefined {
    const extra = this.extra;
    if (this.retentionLive && extra !== undefined && extra.lastToken === token) return extra.lastKey;
    const key = lookupToken(token);
    if (!this.retentionLive) return key;
    if (this.dynamicKey !== undefined) releaseTokenKey(this.dynamicKey);
    this.dynamicKey = undefined;
    const into = this.extra ??= {};
    into.lastToken = key === undefined ? undefined : token; into.lastKey = key;
    if (key !== undefined && !this.retainedKeys?.has(key) && retainTokenKey(key)) this.dynamicKey = key;
    return key;
  }
  /** Releases this definition's references once; retained factory views cannot reacquire ownership. */
  releaseTokens(): void {
    if (!this.retentionLive) return;
    this.retentionLive = false;
    if (this.dynamicKey !== undefined) releaseTokenKey(this.dynamicKey);
    for (const key of this.retainedKeys ?? []) releaseTokenKey(key);
    this.dynamicKey = undefined; this.retainedKeys = undefined;
    if (this.kind === 'multi') {
      const left = (multiKeys.get(this.key) ?? 1) - 1;
      if (left) multiKeys.set(this.key, left); else multiKeys.delete(this.key);
    }
  }
  /** Whether the edge to `binding` is the last one logged from this binding into `pairs`, a container's edge set. */
  observedLast(binding: Binding, pairs: WeakMap<Binding, Set<Binding>> | undefined): boolean {
    const extra = this.extra;
    return pairs !== undefined && extra !== undefined && extra.observedTo === binding && extra.observedIn === pairs;
  }
  /** Remembers the edge to `binding` as the last one logged into `pairs`. A new edge set (after a trim) invalidates it. */
  noteObserved(binding: Binding, pairs: WeakMap<Binding, Set<Binding>>): void {
    const into = this.extra ??= {};
    into.observedTo = binding; into.observedIn = pairs;
  }
  /** How it was registered, such as `singleton` or `registerInstance`. */
  get kindTag(): string { return this.extra?.kindTag ?? KIND_TAGS[this.lifetime][this.kind === 'multi' ? 1 : 0]; }
  get module(): string | undefined { return this.extra?.module; }
  set module(value: string | undefined) { (this.extra ??= {}).module = value; }
  get when(): RegisterOptions['when'] { return this.extra?.when; }
  /** Per-registration deadline, including an explicit null disable value. */
  get timeoutMs(): number | null | undefined { return this.extra?.timeoutMs; }
  /** Lifecycle hooks given with the factory, if any. */
  get lifecycle(): ServiceLifecycle<unknown, unknown> | undefined { return this.extra?.lifecycle; }
  /** Declared dependency edges. */
  get deps(): readonly DependencyEdge[] { return this.extra?.deps ?? NO_DEPS; }
  /** Whether the dependencies were declared. Undeclared bindings are not checked for missing dependencies. */
  get declared(): boolean { return this.extra?.declared ?? false; }
  /** Who disposes the instance. */
  get owner(): BindingDescriptor['owner'] { return this.extra?.owner ?? OWNED; }
  /** Bumped on each replacement of a binding for the same token. */
  get generation(): number { return this.extra?.generation ?? 0; }
  /** Registry view: a single binding stored alone is its own registry entry. */
  get single(): Binding | undefined { return this.kind === 'single' ? this : undefined; }
  /** Registry view: a single binding's entry has no multi registrations. */
  get multi(): readonly Binding[] { return NO_BINDINGS; }
  /** Time of the last change. A binding never changes after registration: a replacement is a new binding. */
  get updatedAt(): number { return this.registeredAt; }
  /** Node identity in the dependency graph: the token key for a single, its own symbol for a multi entry. */
  get graphId(): symbol { return this.kind === 'single' && !this.when ? this.key : ((this.extra ??= {}).graphId ??= Symbol(`candidate:${tokenName(this.token)}`)); }
  /** Id of the binding replaced by this one, while a replacement is staged. */
  get replacesId(): string | undefined { return this.extra?.replacesId; }
  set replacesId(value: string | undefined) { (this.extra ??= {}).replacesId = value; }
  /** The class, for a class registration. */
  get ctor(): Constructor | undefined { return this.extra?.ctor; }
  set ctor(value: Constructor | undefined) { (this.extra ??= {}).ctor = value; }
  /** The class descriptor, for a class registration. */
  get descriptor(): ClassDescriptor | undefined { return this.extra?.descriptor; }
  set descriptor(value: ClassDescriptor | undefined) { (this.extra ??= {}).descriptor = value; }
  /** Unique id of the binding. */
  get id(): string { return (this.extra ??= {}).id ??= 'binding-' + nextSequence(); }
  /** Id of the scope the binding was registered on. */
  get ownerScopeId(): string { return this.ownerContainer.identity.id; }
  /** Handle view: id of the binding. */
  get bindingId(): string { return this.id; }
  /** Handle view: a plain registration retains no instances. */
  get retainedInstances(): readonly T[] { return NO_INSTANCES; }
  /** Handle view: removes this binding. Resolves `false` if it was already removed. */
  unregister(): Promise<boolean> { return this.ownerContainer.unregisterBinding(this.id); }
  /** Handle view: replaces this binding's factory and returns a handle to the replacement. */
  replace(factory: ServiceFactory<T, D> | ServiceLifecycle<T, D>, lifetime: ServiceLifetime = this.lifetime, options?: ReplacementOptions): Promise<BindingHandle<T, D>> {
    return this.ownerContainer.replaceBinding<T, D>(this.id, factory, lifetime, options);
  }
}
/** Binding fields only some bindings have. */
export interface BindingExtra {
  module?: string;
  when?: RegisterOptions['when'];
  /** Per-registration deadline override. */
  timeoutMs?: number | null;
  /** The token the factory resolved last. */
  lastToken?: InjectionToken;
  /** The key of `lastToken`. */
  lastKey?: symbol;
  /** Target of the edge logged last from this binding. */
  observedTo?: Binding;
  /** The edge set it was logged into. */
  observedIn?: WeakMap<Binding, Set<Binding>>;
  /** The id, reserved on first read. */
  id?: string;
  /** How it was registered, when not the lifetime's default tag. */
  kindTag?: string;
  /** Lifecycle hooks given with the factory. */
  lifecycle?: ServiceLifecycle<unknown, unknown>;
  /** Declared dependency edges. */
  deps?: DependencyEdge[];
  /** Whether the dependencies were declared. */
  declared?: boolean;
  /** Who disposes the instance. */
  owner?: BindingDescriptor['owner'];
  /** Replacement generation. */
  generation?: number;
  /** Graph node identity of a multi entry. */
  graphId?: symbol;
  /** Id of the binding replaced, while a replacement is staged. */
  replacesId?: string;
  /** The class, for a class registration. */
  ctor?: Constructor;
  /** The class descriptor, for a class registration. */
  descriptor?: ClassDescriptor;
}

/** A built (or building) instance as the container stores it. */
interface StoredShape extends InstanceRecord {
  /** Registry key of the token. */
  key: symbol;
  /** Whether the binding is a single or a multi registration. */
  kind: 'single' | 'multi';
  /** Unique id of this instance, used in resource snapshots. */
  instanceId: string;
  /** The data the instance was resolved with. */
  data: unknown;
  /** The binding that built the instance. */
  binding: Binding;
  /** The container that holds the instance. */
  container: RecordOwner;
}
/** The id of an instance, assigned on first read: most instances are never shown in a path, edge or snapshot. */
export function idOf(record: StoredRecord): string {
  let id = instanceIds.get(record);
  if (id === undefined) instanceIds.set(record, id = nextResourceInstanceId());
  return id;
}
/** Instance ids, assigned on first read. Kept off the record: most records never show one. */
const instanceIds = new WeakMap<StoredRecord, string>();
/** A built (or building) instance as the container stores it. Its binding id is read through the binding. */
export class StoredRecord implements StoredShape {
  /** Lifecycle state. */
  declare state: InstanceRecord['state'];
  /** The instance, once built. */
  declare instance: unknown;
  /** Time the build started. */
  declare createdAt: number;
  /** Time of the last state change. */
  declare lastTouchedAt: number;
  /** The binding that built the instance. */
  declare readonly binding: Binding;
  /** The container that holds the instance. */
  declare readonly container: RecordOwner;
  /** The data the instance was resolved with. */
  declare readonly data: unknown;
  /** True until materialization is published or cancelled. */
  building = true;
  /** Distinguishes a produced undefined value from an unfinished factory. */
  produced = false;
  /** Cancellation terminal state, distinct from an ordinary factory error. */
  cancelled?: DiError;
  /** Absolute deadline, only for configured asynchronous work. */
  deadline?: number;
  /** Unrefed pending deadline. */
  timer?: ReturnType<typeof setTimeout>;
  /** Shared caller rejection boundary while a build is pending. */
  rejectBuild?: (error: unknown) => void;
  /** Whether exactly-once cleanup has claimed this produced instance. */
  cleanupStarted = false;
  /** Asynchronous cleanup shared by terminal paths. */
  cleanupPromise?: Promise<unknown>;
  /** Lazy build-local cancellation source. */
  private controller?: AbortController;
  /** Stable composed signal, independent of a retained view's resolver state. */
  private signalValue?: AbortSignal;
  /** Terminal abort is retained even when the signal was never read during construction. */
  private abortReason?: DiError;
  /** Stable factory/build signal linked to the owning scope lifetime. */
  get signal(): AbortSignal {
    if (!this.signalValue) {
      this.controller = new AbortController();
      if (this.abortReason) this.controller.abort(this.abortReason);
      this.signalValue = buildSignal(this.controller.signal, this.container.signal);
    }
    return this.signalValue;
  }
  /** Aborts at most once without creating an unused signal or parent listener. */
  abort(reason: DiError): void { this.abortReason ??= reason; this.controller?.abort(this.abortReason); }
  /** Creates the record of a build of `binding` held by `container`. */
  constructor(binding: Binding, container: RecordOwner, data: unknown, at: number) {
    this.state = 'creating'; this.instance = undefined; this.createdAt = this.lastTouchedAt = at;
    this.binding = binding; this.container = container; this.data = data;
  }
  /** Id of the binding that built the instance. */
  get bindingId(): string { return this.binding.id; }
  /** Unique id of this instance, or `''` before one is assigned on first read through `idOf`. */
  get instanceId(): string { return instanceIds.get(this) ?? ''; }
  /** Generation of the binding that built the instance. */
  get generation(): number { return this.binding.generation; }
  /** Registry key of the token. */
  get key(): symbol { return this.binding.key; }
  /** Whether the binding is a single or a multi registration. */
  get kind(): 'single' | 'multi' { return this.binding.kind; }
}

/** Declared dependency metadata, normalized only inside a registration-owned retention scope. */
export type BindingDependency = Omit<DependencyEdge, 'key'>;

/** Lifetime names used in resource snapshots. */
export const lifetimeText: Record<ServiceLifetime, ResourceLifetime> = {
  [ServiceLifetime.Singleton]: 'singleton', [ServiceLifetime.Transient]: 'transient',
  [ServiceLifetime.Scoped]: 'scoped', [ServiceLifetime.Tenant]: 'tenant', [ServiceLifetime.Request]: 'request',
};
/** How long a lifetime lives, for the lifetimes that can be captured by a longer one. Others are 0. */
export const lifespan = (lifetime: ServiceLifetime): number =>
  lifetime === ServiceLifetime.Singleton ? 3 : lifetime === ServiceLifetime.Tenant ? 2 : lifetime === ServiceLifetime.Request ? 1 : 0;

/** Metadata controlled by class registration and replacement staging. */
export interface BindingOptions {
  module?: string;
  when?: RegisterOptions['when'];
  /** Unnormalized edges remain leased by the registration being constructed. */
  deps?: BindingDependency[];
  declared?: boolean;
  owner?: Binding['owner'];
  ctor?: Constructor;
  descriptor?: ClassDescriptor;
  generation?: number;
  timeoutMs?: number | null;
}


/** Runtime construction check narrows erased token unions without pretending abstract tokens are concrete. */
export function isConstructor<T>(token: InjectionToken<T>): token is Constructor<T> {
  if (typeof token !== 'function') return false;
  try { Reflect.construct(Object, [], token); return true; }
  catch { return false; }
}

/** Registrations of one token on one container. */
export interface RegistrationSet {
  singles?: Binding[];
  /** The single registration, if any. */
  single?: Binding;
  /** Multi registrations, in registration order. `NO_BINDINGS` until the first one. */
  multi: Binding[];
}

/** Whether a registration argument is a `ServiceLifecycle` object rather than a bare factory. */
export function isLifecycle<T, D>(factory: ServiceFactory<T, D> | ServiceLifecycle<T, D>): factory is ServiceLifecycle<T, D> {
  return !!factory && typeof factory === 'object' && 'factory' in factory;
}


/** Validates settings before they can silently disable a build deadline. */
export function checkTimeout(value: number | null | undefined, token?: InjectionToken): void {
  if (value !== undefined && value !== null && (typeof value !== 'number' || !Number.isFinite(value) || value < 0)) {
    throw new RangeError(`Invalid build timeout: ${token === undefined ? 'scope' : tokenName(token)}`);
  }
}


  /** Builds a binding record without registering it. */
export function createBinding<T, D>(owner: BindingOwner, token: InjectionToken<T>, factory: ServiceFactory<T, D> | ServiceLifecycle<T, D>,
    lifetime: ServiceLifetime, kind: 'single' | 'multi', kindTag: string, options?: RegisterOptions,
    internal?: BindingOptions): TypedBinding<T, D> {
    const requestedTimeout = options?.timeoutMs;
    const timeoutMs = requestedTimeout === undefined ? internal?.timeoutMs : requestedTimeout;
    checkTimeout(timeoutMs, token);
    const held = new Set<symbol>();
    const own = (dep: InjectionToken): symbol => {
      const key = internToken(dep);
      if (!held.has(key) && retainTokenKey(key)) held.add(key);
      return key;
    };
    try {
      const key = own(token);
      // Storage erases factory data; only the generic registration/resolve boundary supplies it.
      const lifecycle = isLifecycle(factory) ? factory as ServiceLifecycle<unknown, unknown> : undefined;
      const metadata = internal?.deps ?? options?.deps?.map((dep, index) => ({ token: unwrapDependencySlot(dep), lazy: isDeferredToken(dep), index, optional: false, multi: false }));
      const deps = metadata?.map(dep => ({ ...dep, key: own(dep.token) }));
      const declared = internal?.declared ?? (options?.deps !== undefined);
      // A plain registration allocates no extra object: every field it would hold is at its default.
      const extra: BindingExtra | undefined = options?.when || lifecycle || deps || declared || internal || timeoutMs !== undefined || kindTag !== KIND_TAGS[lifetime][kind === 'multi' ? 1 : 0]
        ? { kindTag, lifecycle, deps, declared, timeoutMs, when: options?.when ?? internal?.when, module: internal?.module, owner: internal?.owner, generation: internal?.generation, ctor: internal?.ctor,
          descriptor: internal?.ctor && internal.descriptor }
        : undefined;
      return new TypedBinding<T, D>(owner, token, key, lifetime, kind, (lifecycle ? lifecycle.factory : factory) as AnyFactory, extra);
    } finally { for (const key of held) releaseTokenKey(key); }
  }