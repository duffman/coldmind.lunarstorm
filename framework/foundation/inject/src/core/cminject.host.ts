import type { DependencySlot } from '@coldmind/inject/lazy';
import type { DiGraph } from '@coldmind/inject/graph';
import type { Resolution } from '@coldmind/inject/internal/resolution';
import type { ResolutionView } from '@coldmind/inject/internal/view';
import type {
  ScopeType, ServiceLifetime, ValidatorRecord, AmbientResolution, AutoInjectOptions, BindingHandle, ClassDescriptor,
  DiIssue, DisposalInfo, IContainer, InstanceHook, Interceptor, ParameterInjection, RegisterOptions, ReplacementOptions,
  ScopeIdentity, ServiceFactory, ServiceLifecycle,
} from '@coldmind/inject/contracts';
import type { Constructor, InjectionToken } from '@coldmind/inject/token';
import type { DiError } from '@coldmind/inject/errors';
import type { Binding, StoredRecord, TypedBinding, RegistrationSet } from '@coldmind/inject/internal/binding';
export type { Binding, StoredRecord };

/** Capabilities retained by definitions and public handles, independent of container implementation state. */
export interface BindingOwner {
  /** Stable owning scope identity. */
  readonly identity: ScopeIdentity;
  /** Removes exactly the selected registration. */
  unregisterBinding(bindingId: string): Promise<boolean>;
  /** Replaces exactly the selected typed registration. */
  replaceBinding<T, D = void>(bindingId: string, factory: ServiceFactory<T, D> | ServiceLifecycle<T, D>, lifetime?: ServiceLifetime, options?: ReplacementOptions): Promise<BindingHandle<T, D>>;
}

/** Lifetime-only capabilities retained by stored instances, never the owning container implementation. */
export interface RecordOwner {
  /** Opaque hierarchy identity prevents foreign ambient frames from crossing roots. */
  readonly root: object;
  /** Owning scope abort, including for successfully returned transient signals. */
  readonly signal: AbortSignal;
  /** The teardown reason, when this scope has entered disposal. */
  readonly lifetimeReason: DiError | undefined;
}

/** A build in flight. A cycle across concurrent resolutions is a path through `waitsOn` edges. */
export interface Build {
  /** Binding whose async work this node represents, including transients. */
  readonly binding: Binding;
  /** The builds this build is awaiting right now, with how many awaits are open on each. Edges go once they settle. Created on the first await. */
  waitsOn: Map<Build, number> | undefined;
}
/** One binding being built, linked to the build that asked for it. The linked frames are the resolution path. */
export interface Frame {
  /** The binding being built. */
  readonly binding: Binding;
  /** The container that holds the instance. */
  readonly target: IContainer;
  /** The instance record of this build. */
  readonly record: StoredRecord;
  /** The build that asked for this one. `undefined` at the start of a resolution. */
  readonly parent?: Frame;
}
/** The explicit resolution context, passed through every resolve and into each factory. Replaces ambient async state. */
export interface Resolver {
  /** Admitted work may finish while a replacement drains the graph. */
  readonly operation?: ResolutionOperation;
  /** The innermost build of this resolution. Walk `parent` for the whole path. */
  readonly frame?: Frame;
  /** The build whose factory is running, if any. Each materialization, including a transient, owns a build node. */
  readonly build?: Build;
  /** Bindings being replaced; visible only to the replacement's own builds. */
  readonly staged?: ReadonlyMap<symbol, Binding>;
  /** Skip interceptors. Set for internal resolutions such as scope initialization. */
  readonly suppressInterceptors: boolean;
  /** Explicit completion lineage carried by contextual hook operations. */
  readonly completion?: CompletionLink;
}

/** One external resolution, including nested builds and interceptor callbacks. */
export interface ResolutionOperation {
  readonly done: Promise<void>;
  retain(): void;
  finish(): void;
}

/** Progress of one instance's completion within a container tree. */
export interface CompletionState {
  /** Number of successfully assigned members. */
  members: number;
  /** Whether all wrappers were installed. */
  wrapped: boolean;
  /** Hooks that already finished successfully. */
  hooks: WeakSet<HookSlot>;
  /** Shared asynchronous completion, when work is pending. */
  pending?: Promise<unknown>;
  /** Whether synchronous work is running, including a reentrant hook. */
  active: boolean;
  /** Whether every completion stage succeeded. */
  done: boolean;
  /** Identity of the current attempt, distinct from any retry. */
  attempt?: CompletionAttempt;
}
/** Identity and lifetime of one manual or registered instance-completion attempt. */
export interface CompletionAttempt {
  /** Owning tree. */
  root: WeakMap<object, CompletionState>;
  /** Progress whose current attempt must still match. */
  state: CompletionState;
  /** The instance while the attempt is active. Cleared at settlement. */
  instance?: object;
  /** Whether this attempt can participate in contextual reentry. */
  active: boolean;
}
/** One hook's explicit lineage link; hook settlement expires it independently. */
export interface CompletionLink {
  /** The attempt whose members and wrappers are already applied. */
  attempt: CompletionAttempt;
  /** A caller's completion lineage, if any. */
  parent?: CompletionLink;
  /** Whether this particular hook is still running. */
  valid: boolean;
  /** Resolution context while the hook remains active. Cleared on settlement. */
  context?: () => Resolver;
}

/** What building a class needs from a resolution scope: a container, or the view a factory receives. */
export interface Scope {
  /** Stable build or owner lifetime signal. */
  readonly signal: AbortSignal;
  tryGet<T>(token: InjectionToken<T>): T | undefined;
  /** Explicit completion lineage, absent on a bare container. */
  readonly completion?: CompletionLink;
  /** Active resolution context of a contextual view. */
  resolutionContext?(): Resolver;
  /** Resolves a single token: its value when every step is synchronous, otherwise a promise of it. */
  resolveEither(token: InjectionToken): unknown;
  /** Resolves every multi registration of a token. */
  resolveAll(token: InjectionToken): Promise<unknown[]>;
}

/** An instance hook subscribed with `onInstance`. */
export interface HookSlot {
  /** Decorator key the hook matches. */
  key: string;
  /** The hook to run for each matching instance. */
  hook: InstanceHook;
}


/** Private construction and resolution operations available to a completion facade. */
export interface ContextHost {
  get<T>(context: Resolver, token: InjectionToken<T>): T;
  tryGet<T>(context: Resolver, token: InjectionToken<T>): T | undefined;
  assertMutation(context: Resolver): void;
  /** Normalizes explicit lineage without importing the construction runtime into views. */
  currentCompletion(link?: CompletionLink): CompletionLink | undefined;
  /** Root context for a retained view whose original binding frame has expired. */
  freshContext(completion?: CompletionLink): Resolver;
  /** Validates the implementation side of a constructor registration overload. */
  implementation<T>(token: InjectionToken<T>): Constructor<T>;
  /** Fresh container context when no explicit hook link remains active. */
  context(): Resolver;
  /** Asynchronous contextual single resolution. */
  resolve<T, D>(context: Resolver, token: InjectionToken<T>, data: D | undefined, known?: symbol): Promise<T>;
  /** Synchronous contextual single resolution. */
  resolveSync<T, D>(context: Resolver, token: InjectionToken<T>, data: D | undefined, known?: symbol): T;
  /** Internal sync-or-async slot resolution. */
  resolveEither(context: Resolver, token: InjectionToken, known?: symbol): unknown;
  /** Contextual multi resolution. */
  resolveAll<T, D>(context: Resolver, token: InjectionToken<T>, data: D | undefined): Promise<T[]>;
  /** Applies members with the facade's explicit completion lineage. */
  injectMembers<T extends object>(instance: T, scope: Scope): T;
  /** Asynchronous contextual member completion. */
  injectMembersAsync<T extends object>(instance: T, scope: Scope): Promise<T>;
  /** Constructs an unregistered class with the facade's explicit completion lineage. */
  instantiate<T>(ctor: Constructor<T>, args: readonly unknown[], scope: Scope): T;
  /** Asynchronous contextual construction. */
  instantiateAsync<T>(ctor: Constructor<T>, args: readonly unknown[], scope: Scope): Promise<T>;
}

/** The actual public target, including historically forwarded Container runtime extras. */
export interface ViewTarget extends IContainer {
  registerFactory<T, D>(token: InjectionToken<T>, factory: ServiceFactory<T, D> | ServiceLifecycle<T, D> | import('@coldmind/inject/contracts').InjectedSpec<T> | RegisterOptions | undefined, lifetime: ServiceLifetime, kind: 'single' | 'multi', options?: RegisterOptions): BindingHandle<T, D>;
  /** Tracked teardown is a public Container method, though not part of ordinary IContainer. */
  disposeWithTracking(): Promise<DisposalInfo[]>;
  /** Existing runtime-start rollback remains available on factory views. */
  rollbackNewInstances(preexisting: ReadonlySet<string>): Promise<void>;
}

/** Constructor/slot/member completion authority; no registration or cache mutation operations. */
export interface ConstructionHost {
  /** Rejects work after the owning scope begins disposal. */
  active(): void;
  /** Validates a factory's public facade and exposes only its contextual slot operations. */
  scope(container: IContainer): Scope;
  /** Weak progress owned by the root of this explicit scope. */
  states(scope: Scope): WeakMap<object, CompletionState>;
  /** Ancestor hooks in their established root-first order. */
  hooks(scope: Scope): readonly HookSlot[];
  /** Optional-slot presence check, without registration authority. */
  isRegistered(scope: Scope, token: InjectionToken): boolean;
  /** Fresh root resolution context for a manual scope. */
  context(scope: Scope): Resolver;
  /** Hook-local facade carrying explicit lineage and the originating lifetime/build signal. */
  hookView(scope: Scope, link: CompletionLink): IContainer & Scope;
}

/** Selection/routing/control authority for one owner's resolution pipeline. */
export interface ResolutionHost {
  waitForResolution(context: Resolver): Promise<void> | undefined;
  beginOperation(): ResolutionOperation;
  /** Root-local source and opaque hierarchy identity, inherited by children. */
  readonly ambient: AmbientResolution | undefined;
  readonly root: object;
  /** Scope metadata and inherited policy remain lazy. */
  readonly identity: ScopeIdentity;
  readonly buildTimeout: number | null | undefined;
  readonly recordOwner: RecordOwner;
  /** Active-state check precedes every new materialization. */
  active(): void;
  /** Routes only a selected registration to its lifetime owner. */
  route(binding: Binding): Resolution;
  /** Selects visible definitions, including transaction-local candidates. */
  selectSingle(key: symbol, staged?: ReadonlyMap<symbol, Binding>, context?: Resolver): Binding | undefined;
  selectMulti(key: symbol, staged?: ReadonlyMap<symbol, Binding>): Binding[];
  /** Existing class-only auto-registration policy, not arbitrary factory mutation. */
  autoBinding(token: InjectionToken, key: symbol): Binding | undefined;
  /** Preserves single/multi shadowing semantics before materialization. */
  assertSingle(key: symbol, binding: Binding, token: InjectionToken): void;
  /** Read-only interceptor enumeration. */
  hasInterceptors(): boolean;
  interceptors(): readonly Interceptor[];
  /** Scope metadata preview does not itself build a service. */
  scopeId(binding: Binding): string | undefined;
  /** View creation is the sole public-target boundary; helpers never receive private container state. */
  view(binding: Binding, record: StoredRecord, parent: Frame | undefined, context: Resolver): ResolutionView;
  /** Observes selected definitions and successful resource links. */
  observe(from: Binding, to: Binding): void;
  observeResource(from: StoredRecord, to: StoredRecord): void;
  /** Central lifecycle ownership/cleanup claim, shared with teardown. */
  dispose(record: StoredRecord, unreturned: boolean): unknown;
  /** Root diagnostic snapshot includes pending transients too. */
  allBuilds(): readonly ResolutionView[];
  /** Public errors pin only the issue keys actually exposed. */
  exposeIssues(issues: DiIssue[]): DiIssue[];
}


/** A dependency between two bindings, observed during resolution. */
export interface Observed {
  /** Id of the binding that resolved the dependency. */
  from: string;
  /** Id of the binding that was resolved. */
  to: string;
  /** Generation of the resolving binding at the time. */
  fromGeneration: number;
  /** Generation of the resolved binding at the time. */
  toGeneration: number;
  /** `declared` when the resolving binding lists the dependency, `dynamic` otherwise. */
  type: 'declared' | 'dynamic';
  /** The scope the resolution ran in. */
  scopeId: string;
  /** When the edge was observed. */
  observedAt: number;
}

/** A dependency between two live instances, observed during resolution. */
export interface ResourceObserved {
  /** Instance id of the dependent. */
  from: string;
  /** Instance id of the dependency. */
  to: string;
  /** The scope the resolution ran in. */
  scopeId: string;
  /** When the edge was observed. */
  observedAt: number;
}
/** Exact scope state needed while staging/rolling back one mutation transaction. */
export interface MutationScope {
  readonly scopeType: ScopeType;
  readonly order: readonly Binding[];
  readonly records: Map<Binding, StoredRecord>;
  readonly pending: Map<Binding, Promise<StoredRecord>>;
  readonly disposalOrder: Binding[];
  readonly observed: Observed[];
  readonly resourceObserved: ResourceObserved[];
  readonly graphIssues: DiIssue[];
  readonly graphIssueKeys: symbol[][];
  readonly ended: boolean;
  epoch: number;
  /** Existing resolution/control engine is the only way candidates materialize. */
  entry(binding: Binding, data: unknown, context: Resolver): StoredRecord | Promise<StoredRecord>;
  route(binding: Binding): MutationScope;
  indexGraph(): void;
  forgetObserved(): void;
  restoreGraphIssues(issues: readonly DiIssue[], keys: readonly symbol[][]): void;
}
/** Registry mutation authority; identity/generation/commit remain centralized here. */
export interface MutationHost extends MutationScope {
  transaction<T>(run: (staged: Map<symbol, Binding>) => Promise<T>): Promise<T>;
  beginOperation(): ResolutionOperation;
  remapBinding(bindingId: string, handle: Pick<BindingHandle, 'bindingId' | 'unregister'>): void;
  readonly owner: BindingOwner;
  readonly registry: Map<symbol, Binding | RegistrationSet>;
  readonly order: Binding[];
  readonly parent: MutationHost | undefined;
  readonly autoInject: boolean | AutoInjectOptions;
  readonly lifetimeReason: DiError | undefined;
  /** Eager supplied-instance registration still creates records through the one resolution owner. */
  guardMutation(bindingId: string): void;
  factoryArgs(container: IContainer, slots: readonly DependencySlot[]): unknown[] | Promise<unknown[]>;
  makeRecord(binding: Binding, data: unknown): StoredRecord;
  active(context?: Resolver): void;
  subtree(): MutationScope[];
  view(): MutationScope[];
  nearestScope(kind: ScopeType): MutationScope | undefined;
  recordScope(record: StoredRecord): MutationScope;
  selectMulti(key: symbol): Binding[];
  localSingle(key: symbol): Binding | undefined;
  isRegisteredRecursive(token: InjectionToken): boolean;
  replace<T, D = void>(token: InjectionToken<T>, factory: ServiceFactory<T, D> | ServiceLifecycle<T, D>, lifetime: ServiceLifetime, options?: ReplacementOptions): Promise<BindingHandle<T, D>>;
  classFactory<T, D>(ctor: Constructor<T>, descriptor: ClassDescriptor, params: readonly ParameterInjection[]): ServiceFactory<T, D>;
  handle<T, D>(binding: TypedBinding<T, D>, retained?: readonly T[]): BindingHandle<T, D>;
  disposeRecord(record: StoredRecord, tracked: boolean): Promise<DisposalInfo | undefined>;
  rebuildGraphKey(key: symbol): void;
  finishGraphIndex(): void;
}

/** Read-only scope projection for runtime/declared graph diagnostics. */
export interface ObserveScope {
  readonly parent: ObserveScope | undefined;
  readonly identity: ScopeIdentity;
  readonly epoch: number;
  readonly owner: BindingOwner | undefined;
  readonly registry: ReadonlyMap<symbol, Binding | RegistrationSet>;
  readonly order: readonly Binding[];
  readonly records: ReadonlyMap<Binding, StoredRecord>;
  readonly observed: readonly Observed[];
  readonly resourceObserved: readonly ResourceObserved[];
  ancestors(): ObserveScope[];
  selectSingle(key: symbol): Binding | undefined;
}
/** Observation can read exact definitions/records and run existing public validators, never install definitions. */
export interface ObserveHost extends ObserveScope {
  active(): void;
  view(): ObserveScope[];
  descendants(): ObserveScope[];
  ownerOf(binding: Binding): ObserveScope;
  selectMulti(key: symbol): Binding[];
  isRegisteredRecursive(token: InjectionToken): boolean;
  parentGraph(): DiGraph | undefined;
  validator(binding: Binding, validator: ValidatorRecord): ReturnType<ValidatorRecord['check']>;
}
