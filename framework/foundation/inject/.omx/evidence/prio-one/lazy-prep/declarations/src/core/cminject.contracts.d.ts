import type { Constructor, InjectionToken } from '@coldmind/inject/src/core/cminject.token.ts';
import type { Dialect, Placement } from '@coldmind/decorate';
/** How long an instance lives: singleton, transient, scoped (per resolving container), tenant or request. */
export declare enum ServiceLifetime {
    Singleton = 0,
    Transient = 1,
    Scoped = 2,
    Tenant = 3,
    Request = 4
}
/** Kind of a scope: the root, a tenant, a request, or a generic child. */
export declare enum ScopeType {
    Global = "global",
    Tenant = "tenant",
    Request = "request",
    Generic = "generic"
}
/** Builds a service. Receives the container and, when the service takes data, that data. */
export type ServiceFactory<T, TData = void> = TData extends void ? (c: IContainer) => T | Promise<T> : (c: IContainer, data: TData) => T | Promise<T>;
/** Result of a health check. */
export interface HealthStatus {
    /** Whether the service is healthy. */
    healthy: boolean;
    /** Why it is not healthy. */
    error?: string;
}
/** A factory with optional lifecycle hooks, used in place of a bare factory. */
export interface ServiceLifecycle<T, TData = void> {
    /** The factory that builds the service. */
    factory: ServiceFactory<T, TData>;
    /** Runs after the factory, before the instance is handed out. */
    onInit?(instance: T, c: IContainer): void | Promise<void>;
    /** Runs when the owning container disposes the instance. */
    onDispose?(instance: T): void | Promise<void>;
    /** Reports the instance's health for `healthCheck`. */
    onHealthCheck?(instance: T): HealthStatus | Promise<HealthStatus>;
}
/** Optional lifecycle hooks a service may implement without decorators. */
export interface IInjectable {
    /** Runs after construction and injection, before the instance is handed out. */
    onInit?(): void | Promise<void>;
    /** Runs once the owning scope has initialized all its services. */
    onReady?(): void | Promise<void>;
    /** Runs when the owning container disposes the instance. */
    onDispose?(): void | Promise<void>;
    /** Reports the instance's health for `healthCheck`. */
    onHealthCheck?(): HealthStatus | Promise<HealthStatus>;
}
/** Options for a registration. */
export interface RegisterOptions {
    /** Async build deadline in milliseconds; undefined inherits, null disables, zero schedules immediate expiry. */
    timeoutMs?: number | null;
    /** Declared dependencies. When given, the graph checks them for missing, cycle and captive problems. */
    deps?: readonly InjectionToken[];
}
/** Options for replacing a registration. */
export interface ReplacementOptions {
    /** Keep the old instances alive and return them on the handle instead of disposing them. */
    retainPrevious?: boolean;
    /** Reject with `MutationConflictError` when the binding has a built or building instance. */
    rejectIfLive?: boolean;
    /** Build the replacement and rebuild live dependents right away. */
    rebuild?: boolean;
    /** Declared dependencies of the replacement. Defaults to the old binding's. */
    deps?: readonly InjectionToken[];
}
/** Identity of a scope: its id, kind, and tenant or request id when it has one. */
export interface ScopeIdentity {
    /** Unique scope id. */
    id: string;
    /** Kind of scope. */
    kind: 'global' | 'tenant' | 'request' | 'generic';
    /** Tenant id, set on a tenant scope and inherited by its descendants. */
    tenantId?: string;
    /** Request id, set on a request scope and inherited by its descendants. */
    requestId?: string;
    /** Id of a binding that owns this scope. The container itself never sets it. */
    ownerBindingId?: string;
}
/** Handle returned by a registration, for unregistering or replacing it. */
export interface BindingHandle<T = unknown, TData = void> {
    /** Id of the binding. */
    readonly bindingId: string;
    /** The registered token. */
    readonly token: InjectionToken<T>;
    /** Whether this is a single or a multi registration. */
    readonly kind: 'single' | 'multi';
    /** Generation of the binding. A replacement increments it. */
    readonly generation: number;
    /** Instances of the previous binding kept by `retainPrevious`. Empty otherwise. */
    readonly retainedInstances: ReadonlyArray<T>;
    /** Removes this binding. Resolves `false` if it was already removed. */
    unregister(): Promise<boolean>;
    /** Replaces this binding's factory and returns a handle to the replacement. */
    replace(factory: ServiceFactory<T, TData> | ServiceLifecycle<T, TData>, lifetime?: ServiceLifetime, options?: ReplacementOptions): Promise<BindingHandle<T, TData>>;
}
/** Lifecycle state of a built instance. */
export type InstanceState = 'uninitialized' | 'creating' | 'initializing' | 'ready' | 'disposing' | 'disposed' | 'failed';
/** Snapshot of a built instance. */
export interface InstanceRecord<T = unknown> {
    /** Id of the binding that built the instance. */
    bindingId: string;
    /** Generation of that binding. */
    generation: number;
    /** Lifecycle state. */
    state: InstanceState;
    /** The instance, once constructed. */
    instance?: T;
    /** When the build started. */
    createdAt: number;
    /** When the instance last changed state or was resolved from the cache. */
    lastTouchedAt: number;
}
/** Public description of a registration. */
export interface BindingDescriptor<T = unknown> {
    /** Unique binding id. */
    id: string;
    /** Generation of the binding. A replacement increments it. */
    generation: number;
    /** Id of the scope the binding was registered on. */
    ownerScopeId: string;
    /** The registered token. */
    token: InjectionToken<T>;
    /** How long instances live. */
    lifetime: ServiceLifetime;
    /** Whether this is a single or a multi registration. */
    kind: 'single' | 'multi';
    /** How the binding was made, such as the registration method or `replace`. */
    kindTag: string;
    /** Whether the container owns (and disposes) the instances, and why when it does not. */
    owner: {
        ownership: 'owned' | 'external';
        reason?: string;
    };
    /** When the binding was registered. */
    registeredAt: number;
    /** When the binding last changed. */
    updatedAt: number;
}
/** A multi-registration with its position in resolution order. */
export interface MultiBindingEntry<T = unknown> extends BindingDescriptor<T> {
    /** Position in resolution order. */
    order: number;
}
/** A built instance with the descriptor of its binding. */
export interface CacheableBinding<T = unknown> extends BindingDescriptor<T> {
    /** The built instance. */
    record: InstanceRecord<T>;
}
/** A scope in the ancestor chain, with its depth and parent. */
export interface ScopeTreeNode {
    /** The scope. */
    scope: ScopeIdentity;
    /** Distance from the root. The root is 0. */
    depth: number;
    /** The parent scope, absent for the root. */
    parent?: ScopeIdentity;
}
/** A registry epoch with the scope that owns it. */
export interface RuntimeEpochSnapshot {
    /** The registry epoch. */
    epoch: number;
    /** When the snapshot was taken. */
    createdAt: number;
    /** Id of the scope whose epoch this is. */
    ownerScopeId: string;
}
/** A ready instance held by a scope. */
export interface OwnershipRecord {
    /** Id of the scope holding the instance. */
    scopeId: string;
    /** Id of the binding that built it. */
    bindingId: string;
    /** Generation of that binding. */
    generation: number;
}
/** A reference to one generation of a binding. */
export interface RuntimeBindingRef {
    /** Id of the binding. */
    bindingId: string;
    /** Generation of the binding. */
    generation: number;
}
/** An observed dependency between two binding generations. */
export interface RuntimeEdge {
    /** The dependent binding. */
    from: RuntimeBindingRef;
    /** The dependency. */
    to: RuntimeBindingRef;
    /** `declared` when the dependent lists the dependency, `dynamic` otherwise. */
    type: 'declared' | 'dynamic';
}
/** One step of a resolution path: the binding and the scope it was built in. */
export interface ResolutionPathStep {
    /** Id of the binding being built. */
    bindingId: string;
    /** Id of the instance being built, once its record exists. */
    instanceId?: string;
    /** Readable name of the token. */
    tokenName: string;
    /** Id of the scope that holds the instance. */
    scopeId: string;
}
/** What an interceptor sees about one resolution. */
export interface ResolutionContext<T = unknown, TData = unknown> {
    /** Registry key of the resolved token. */
    token: symbol;
    /** Lifetime of the binding. `Transient` when the token is not registered. */
    lifetime: ServiceLifetime;
    /** Data passed to `resolve`. */
    data?: TData;
    /** Attempt number, starting at 1. */
    attempt: number;
    /** Resolution path including this binding, outermost first. */
    readonly path?: readonly ResolutionPathStep[];
    /** The step that asked for this token, absent for a top-level resolve. */
    readonly requester?: ResolutionPathStep;
}
/** Observes resolutions. All hooks are optional, and an error from a hook is attached to the resolution error. */
export interface Interceptor {
    /** Runs before the binding is looked up. */
    onBeforeResolve?<T>(ctx: ResolutionContext<T>): void | Promise<void>;
    /** Runs after the instance is ready, with the time the resolution took. */
    onAfterResolve?<T>(ctx: ResolutionContext<T>, instance: T, durationMs: number): void | Promise<void>;
    /** Runs when the resolution fails. */
    onResolutionError?<T>(ctx: ResolutionContext<T>, error: Error): void | Promise<void>;
}
/** Outcome of disposing one service. */
export interface DisposalInfo {
    /** Registry key of the disposed service. */
    token: symbol;
    /** Outcome of the disposal. */
    status: 'pending' | 'disposing' | 'disposed' | 'failed';
    /** Time the disposal hook took. */
    durationMs?: number;
    /** The error thrown by the disposal hook. */
    error?: Error;
}
/** A binding in a dependency snapshot. */
export interface RuntimeDependencyNode {
    /** Id of the binding. */
    bindingId: string;
    /** Readable name of the token. */
    token: string;
    /** Id of the scope the binding was registered on. */
    ownerScopeId: string;
    /** Id of the scope the snapshot was taken from. */
    scopeId: string;
    /** Whether this is a single or a multi registration. */
    kind: 'single' | 'multi';
    /** How the binding was made. */
    kindTag: string;
    /** Generation of the binding. */
    generation: number;
    /** When the binding was registered. */
    registeredAt: number;
    /** When the binding last changed. */
    updatedAt: number;
}
/** A dependency between two bindings in a snapshot. */
export interface RuntimeDependencyEdge {
    /** Id of the dependent binding. */
    from: string;
    /** Id of the dependency. */
    to: string;
    /** `declared` when listed in the registration, `runtime` when only observed during resolution. */
    type: 'declared' | 'runtime';
    /** Id of the scope the edge belongs to. */
    scopeId: string;
}
/** Bindings and the dependencies between them. */
export interface RuntimeDependencyGraph {
    /** The bindings. */
    nodes: RuntimeDependencyNode[];
    /** The dependencies between them. */
    edges: RuntimeDependencyEdge[];
}
/** A node of a dependency tree. */
export interface RuntimeDependencyTreeNode {
    /** Id of the binding. */
    bindingId: string;
    /** Readable name of the token. */
    token: string;
    /** Id of the scope the binding was registered on. */
    scopeId: string;
    /** Dependencies of this binding. Empty for a binding already shown higher up. */
    children: RuntimeDependencyTreeNode[];
}
/** Lifetime name used in resource snapshots. */
export type ResourceLifetime = 'singleton' | 'transient' | 'scoped' | 'tenant' | 'request';
/** A live instance in a resource snapshot. */
export interface RuntimeResourceNode {
    /** Id of the instance. */
    instanceId: string;
    /** Id of the binding that built it. */
    bindingId: string;
    /** Generation of that binding. */
    generation: number;
    /** Readable name of the token. */
    token: string;
    /** Lifetime of the binding. */
    lifetime: ResourceLifetime;
    /** Id of the scope the binding was registered on. */
    ownerScopeId: string;
    /** Id of the scope that holds the instance. */
    resolutionScopeId: string;
    /** Tenant id of the holding scope. */
    tenantId?: string;
    /** Request id of the holding scope. */
    requestId?: string;
    /** Whether the container disposes the instance. */
    ownership: 'owned' | 'external';
    /** Lifecycle state. */
    state: InstanceState;
    /** When the build started. */
    createdAt: number;
    /** When the instance last changed state or was resolved from the cache. */
    lastTouchedAt: number;
}
/** A runtime dependency between two live instances. */
export interface RuntimeResourceEdge {
    /** Instance id of the dependent. */
    from: string;
    /** Instance id of the dependency. */
    to: string;
    /** Always `runtime`: resource edges are only observed. */
    type: 'runtime';
    /** Id of the scope the resolution ran in. */
    scopeId: string;
    /** When the edge was observed. */
    observedAt: number;
}
/** Scopes, bindings, live instances and the edges between them. */
export interface RuntimeResourceGraph {
    /** Sum of the registry epochs of the scopes in the snapshot. */
    epoch: number;
    /** The scopes in the snapshot. */
    scopes: ScopeIdentity[];
    /** The bindings visible from those scopes. */
    bindings: RuntimeDependencyNode[];
    /** Dependencies between the bindings. */
    bindingEdges: RuntimeDependencyEdge[];
    /** Live instances. */
    resources: RuntimeResourceNode[];
    /** Dependencies between live instances. */
    resourceEdges: RuntimeResourceEdge[];
}
/** A declared dependency of a binding: the token it needs and where it is injected. */
export interface DependencyEdge {
    /** The token depended on. */
    token: InjectionToken;
    /** Registry key of that token. */
    key: symbol;
    /** Constructor parameter index, or position in the declared list. */
    index: number;
    /** A missing provider is not an error. */
    optional: boolean;
    /** Depends on every multi registration of the token. */
    multi: boolean;
    /** Name of the injected property, for a member injection. */
    property?: string | symbol;
}
/** A binding as the dependency graph sees it. */
export interface DiNode {
    /** Registry key of the token the node provides. */
    key: symbol;
    /** Node identity when it differs from `key`, as for each multi entry. */
    identity?: symbol;
    /** The token the node provides. */
    token: InjectionToken;
    /** Lifetime of the binding. */
    lifetime: ServiceLifetime;
    /** Declared dependencies. */
    deps: readonly DependencyEdge[];
    /** Whether the dependencies were declared. Undeclared nodes are not checked for missing dependencies. */
    declared: boolean;
}
/** Kind of a graph problem: a missing dependency, a cycle, a captive lifetime, a dangling edge or a decorator error. */
export type DiIssueKind = 'missing' | 'cycle' | 'captive' | 'dangling' | 'decorator';
/** A problem found in the dependency graph. */
export interface DiIssue {
    /** Kind of problem. */
    kind: DiIssueKind;
    /** Readable description. */
    message: string;
    /** Key of the node where the problem was found. */
    at: symbol;
    /** The edge at fault, for missing, captive and dangling problems. */
    edge?: DependencyEdge;
    /** Keys along the cycle, for a cycle. */
    path?: readonly symbol[];
}
/** A dependency listed in `$inject` or a registration: a token, or a token with `optional` or `all`. */
export type StaticInjection = InjectionToken | {
    token: InjectionToken;
    optional?: boolean;
    all?: boolean;
};
/** What a decorator or static declaration contributes to a registration. */
export interface RegistrationContribution {
    /** Lifetime to register with. */
    lifetime?: ServiceLifetime;
    /** Token to register under. Defaults to the class. */
    token?: InjectionToken;
    /** Constructor dependencies, in parameter order. */
    deps?: readonly StaticInjection[];
    /** Register as a multi registration. */
    multi?: boolean;
}
/** Registration derived from a class, with the source that produced it. */
export interface ClassRegistration extends RegistrationContribution {
    /** What produced the registration: a decorator name or a static declaration. */
    source: string;
}
/** A constructor parameter to inject. */
export interface ParameterInjection {
    /** Constructor parameter index. */
    index: number;
    /** Token to inject. Absent when only reflected metadata gives it. */
    token?: InjectionToken;
    /** Inject `undefined` when the token is not registered. */
    optional: boolean;
    /** Inject every multi registration of the token. */
    multi: boolean;
}
/** A property to inject after construction. */
export interface MemberInjection {
    /** Name of the property. */
    name: string | symbol;
    /** Token to inject. */
    token: InjectionToken;
    /** Leave the property `undefined` when the token is not registered. */
    optional: boolean;
    /** Inject every multi registration of the token. */
    multi: boolean;
    /** Maps the resolved value before it is assigned. */
    derive?: (value: unknown, instance: object) => unknown;
    /** What declared the injection: a decorator name or a static declaration. */
    source: string;
    /** Assigns the value on an instance. */
    set: (instance: object, value: unknown) => void;
}
/** A method marked by a decorator. */
export interface MethodRecord {
    /** Name of the method. */
    name: string | symbol;
    /** Name of the decorator that marked it. */
    decorator: string;
    /** Options given to the decorator. */
    options: unknown;
    /** Returns the method bound to an instance. */
    get: (instance: object) => (...args: unknown[]) => unknown;
}
/** Wraps a method, receiving the original and the instance. */
export type MethodWrap = (original: (...args: unknown[]) => unknown, ctx: {
    instance: object;
    name: string | symbol;
}) => (...args: unknown[]) => unknown;
/** A method wrapper contributed by a decorator. */
export interface WrapRecord {
    /** Name of the wrapped method. */
    name: string | symbol;
    /** Name of the decorator that wraps it. */
    decorator: string;
    /** The wrapper. */
    wrap: MethodWrap;
}
/** Everything a class declares for injection: registration, parameters, members, methods, wrappers, validators and tags. */
export interface ClassDescriptor {
    /** Registration data, when the class declares any. */
    registration?: ClassRegistration;
    /** Constructor parameters to inject. */
    params: ParameterInjection[];
    /** Properties to inject after construction. */
    members: MemberInjection[];
    /** Methods marked for discovery. */
    methods: MethodRecord[];
    /** Method wrappers applied at instance creation. */
    wraps: WrapRecord[];
    /** Checks run when the class is registered. */
    validators: ValidatorRecord[];
    /** Tags `discover` can match. */
    tags: string[];
}
/** What a decorator validator receives. */
export interface ValidationContext {
    /** The class being registered. */
    ctor: Constructor;
    /** Its merged descriptor. */
    descriptor: ClassDescriptor;
    /** The container it is registered on. */
    container: IContainer;
}
/** Checks a class. Returns an error message, a list of them, or `undefined` when the class is valid. */
export type DecoratorValidator = (ctx: ValidationContext) => string | readonly string[] | undefined;
/** A validator contributed by a decorator. */
export interface ValidatorRecord {
    /** Name of the decorator that added the check. */
    decorator: string;
    /** The check. */
    check: DecoratorValidator;
}
/** Where a decorator is applied. */
export type DecoratorPlacement = Placement;
/** Decorator dialect: legacy (`experimentalDecorators`) or standard (TC39). */
export type DecoratorDialect = Dialect;
/** What a decorator writes to. Each decorator calls these methods to contribute to the class descriptor. */
export interface DecorationTarget {
    /** Which decorator dialect is applying. */
    dialect: DecoratorDialect;
    /** Where the decorator is applied. */
    placement: DecoratorPlacement;
    /** Name of the decorated member. `undefined` for a class or constructor parameter. */
    name: string | symbol | undefined;
    /** Parameter index for a parameter decorator. `undefined` otherwise. */
    index: number | undefined;
    /** Whether the decorated member is static. */
    static: boolean;
    /** Contributes registration data (lifetime, token, dependencies, multi). */
    register(spec: RegistrationContribution): void;
    /** Adds a tag that `discover` can match. */
    tag(tag: string): void;
    /** Declares an injected dependency for a parameter or member. */
    inject(spec: {
        token: InjectionToken;
        optional?: boolean;
        all?: boolean;
        derive?: (v: unknown, inst: object) => unknown;
    }): void;
    /** Marks the decorated method for discovery. */
    method(options?: unknown): void;
    /** Wraps the decorated method at instance creation. */
    wrap(wrap: MethodWrap): void;
    /** Adds a check that runs when the class is registered. */
    validate(check: DecoratorValidator): void;
}
/** Definition of a decorator: its name, where it may be applied, and how it applies. */
export interface DecoratorSpec<TArgs extends unknown[]> {
    /** Name used in error messages, such as `@Inject`. */
    name: string;
    /** Placements where the decorator is allowed. Others throw. */
    on: readonly DecoratorPlacement[];
    /** Applies the decorator by calling methods on the target. */
    apply(target: DecorationTarget, ...args: TArgs): void;
}
/** A decorator that works in every placement and dialect. */
export type UniversalDecorator = ClassDecorator & PropertyDecorator & MethodDecorator & ParameterDecorator & ((value: any, context: any) => any);
/** Options for `@Injectable`. */
export type InjectableOptions = RegistrationContribution & {
    tags?: readonly string[];
};
/** Options for a lifetime decorator. */
export type LifetimeOptions = Omit<InjectableOptions, 'lifetime'>;
/** Type of the `@Injectable` decorator. */
export type InjectableDecorator = UniversalDecorator;
/** Type of the `@Inject` decorator family. */
export type InjectDecorator = UniversalDecorator;
/**
 * Runs after injected members and wrappers are applied. Use `ctx.container` for reentrant async injection or
 * construction: it carries an explicit hook-local completion context. Captured bare containers join pending work;
 * awaiting one's own completion through a captured container creates an unsupported user-promise cycle.
 */
export type InstanceHook = (instance: object, ctx: {
    methods: MethodRecord[];
    descriptor: ClassDescriptor;
    container: IContainer;
}) => void | Promise<void>;
/** A class found by `discover`, with its binding and matching methods. */
export interface DiscoveredClass {
    /** The binding that registers the class. */
    binding: BindingDescriptor;
    /** The class. */
    ctor: Constructor;
    /** Its methods marked with the matching key. */
    methods: MethodRecord[];
    /** Its tags. */
    tags: string[];
}
/** Settings for `autoInject`. */
export interface AutoInjectOptions {
    /** Lifetime of a class registered on demand. A class with its own registration keeps that one. Default: transient. */
    lifetime?: ServiceLifetime;
}
/** Public abstraction received by factories and lifecycle hooks. */
export interface IContainer {
    /** Async build deadline inherited by children; undefined inherits and null explicitly disables it. */
    buildTimeout: number | null | undefined;
    /** Build signal in a factory/hook context, otherwise this container's lifetime signal. */
    readonly signal: AbortSignal;
    /** The scope kind. */
    readonly scopeType: ScopeType;
    /** True once `dispose` has run on this container. */
    readonly isDisposed: boolean;
    /** Identity of this scope: id, kind, and tenant or request id. */
    readonly identity: ScopeIdentity;
    /** Creates a generic child scope. */
    createScope(scopeId?: string): IContainer;
    /** Creates a tenant scope. Tenant services are built once per tenant scope. */
    createTenantScope(tenantId?: string): IContainer;
    /** Creates a request scope. Request services are built once per request scope. */
    createRequestScope(requestId?: string): IContainer;
    /** Registers a singleton. */
    singleton<T, TData = void>(token: InjectionToken<T>, factory: ServiceFactory<T, TData> | ServiceLifecycle<T, TData>, options?: RegisterOptions): BindingHandle<T, TData>;
    /** The class is its own token and implementation. `options.deps` are its constructor arguments; otherwise its injection metadata is used. */
    singleton<T>(ctor: Constructor<T>, options?: RegisterOptions): BindingHandle<T>;
    /** Registers a transient. */
    transient<T, TData = void>(token: InjectionToken<T>, factory: ServiceFactory<T, TData> | ServiceLifecycle<T, TData>, options?: RegisterOptions): BindingHandle<T, TData>;
    /** The class is its own token and implementation. `options.deps` are its constructor arguments; otherwise its injection metadata is used. */
    transient<T>(ctor: Constructor<T>, options?: RegisterOptions): BindingHandle<T>;
    /** Registers a scoped service. */
    scoped<T, TData = void>(token: InjectionToken<T>, factory: ServiceFactory<T, TData> | ServiceLifecycle<T, TData>, options?: RegisterOptions): BindingHandle<T, TData>;
    /** The class is its own token and implementation. `options.deps` are its constructor arguments; otherwise its injection metadata is used. */
    scoped<T>(ctor: Constructor<T>, options?: RegisterOptions): BindingHandle<T>;
    /** Registers a tenant-scoped service. */
    tenant<T, TData = void>(token: InjectionToken<T>, factory: ServiceFactory<T, TData> | ServiceLifecycle<T, TData>, options?: RegisterOptions): BindingHandle<T, TData>;
    /** The class is its own token and implementation. `options.deps` are its constructor arguments; otherwise its injection metadata is used. */
    tenant<T>(ctor: Constructor<T>, options?: RegisterOptions): BindingHandle<T>;
    /** Registers a request-scoped service. */
    request<T, TData = void>(token: InjectionToken<T>, factory: ServiceFactory<T, TData> | ServiceLifecycle<T, TData>, options?: RegisterOptions): BindingHandle<T, TData>;
    /** The class is its own token and implementation. `options.deps` are its constructor arguments; otherwise its injection metadata is used. */
    request<T>(ctor: Constructor<T>, options?: RegisterOptions): BindingHandle<T>;
    /** Adds a singleton multi-registration. */
    singletonMulti<T, TData = void>(token: InjectionToken<T>, factory: ServiceFactory<T, TData> | ServiceLifecycle<T, TData>, options?: RegisterOptions): BindingHandle<T, TData>;
    /** The class is its own token and implementation. `options.deps` are its constructor arguments; otherwise its injection metadata is used. */
    singletonMulti<T>(ctor: Constructor<T>, options?: RegisterOptions): BindingHandle<T>;
    /** Adds a transient multi-registration. */
    transientMulti<T, TData = void>(token: InjectionToken<T>, factory: ServiceFactory<T, TData> | ServiceLifecycle<T, TData>, options?: RegisterOptions): BindingHandle<T, TData>;
    /** The class is its own token and implementation. `options.deps` are its constructor arguments; otherwise its injection metadata is used. */
    transientMulti<T>(ctor: Constructor<T>, options?: RegisterOptions): BindingHandle<T>;
    /** Adds a scoped multi-registration. */
    scopedMulti<T, TData = void>(token: InjectionToken<T>, factory: ServiceFactory<T, TData> | ServiceLifecycle<T, TData>, options?: RegisterOptions): BindingHandle<T, TData>;
    /** The class is its own token and implementation. `options.deps` are its constructor arguments; otherwise its injection metadata is used. */
    scopedMulti<T>(ctor: Constructor<T>, options?: RegisterOptions): BindingHandle<T>;
    /** Adds a tenant-scoped multi-registration. */
    tenantMulti<T, TData = void>(token: InjectionToken<T>, factory: ServiceFactory<T, TData> | ServiceLifecycle<T, TData>, options?: RegisterOptions): BindingHandle<T, TData>;
    /** The class is its own token and implementation. `options.deps` are its constructor arguments; otherwise its injection metadata is used. */
    tenantMulti<T>(ctor: Constructor<T>, options?: RegisterOptions): BindingHandle<T>;
    /** Adds a request-scoped multi-registration. */
    requestMulti<T, TData = void>(token: InjectionToken<T>, factory: ServiceFactory<T, TData> | ServiceLifecycle<T, TData>, options?: RegisterOptions): BindingHandle<T, TData>;
    /** The class is its own token and implementation. `options.deps` are its constructor arguments; otherwise its injection metadata is used. */
    requestMulti<T>(ctor: Constructor<T>, options?: RegisterOptions): BindingHandle<T>;
    /**
     * Registers an existing instance as a singleton. It is reported as owned and disposed with the container unless
     * `ownerPolicy` says `external`. `clearInstances` keeps it either way.
     */
    registerInstance<T>(token: InjectionToken<T>, instance: T, ownerPolicy?: {
        ownership: 'owned' | 'external';
        reason?: string;
    }): BindingHandle<T>;
    /** Registers a class using its descriptor. */
    registerClass<T>(ctor: Constructor<T>): BindingHandle<T>;
    /** Resolves a single service. Rejects when the token is unregistered or its build fails. */
    resolve<T, TData = void>(token: InjectionToken<T>, data?: TData): Promise<T>;
    /** Resolves a single service synchronously. Throws `LifecycleError` when any step of the build is asynchronous. */
    resolveSync<T, TData = void>(token: InjectionToken<T>, data?: TData): T;
    /** Resolves every multi-registration for a token, in order. */
    resolveAll<T, TData = void>(token: InjectionToken<T>, data?: TData): Promise<T[]>;
    /**
     * Builds `ctor`: `args` fill its first constructor parameters, the container resolves the rest, then applies the class's
     * members, wraps and instance hooks. The instance is not registered, cached or disposed by the container.
     * Throws `LifecycleError` when a dependency is asynchronous; use `instantiateAsync`.
     */
    instantiate<T>(ctor: Constructor<T>, ...args: unknown[]): T;
    /** As `instantiate`, for classes with asynchronous dependencies. */
    instantiateAsync<T>(ctor: Constructor<T>, ...args: unknown[]): Promise<T>;
    /**
     * Applies the members, wraps and instance hooks its class declares to an instance the container did not construct.
     * An instance is completed once; later calls return it unchanged. Throws `LifecycleError` when a dependency is asynchronous.
     */
    injectMembers<T extends object>(instance: T): T;
    /** As `injectMembers`, for classes with asynchronous or multi-registered dependencies. */
    injectMembersAsync<T extends object>(instance: T): Promise<T>;
    /**
     * Builds unregistered classes on demand. When on, resolving a class token that has no registration registers the class
     * on the resolving container (`kindTag: 'auto'`) and resolves it. A scope inherits the setting until it sets its own.
     */
    autoInject: boolean | AutoInjectOptions;
    /** Synchronous read of a built singleton. Throws when it is not built. */
    get<T>(token: InjectionToken<T>): T;
    /** Synchronous read of a built singleton, or `undefined`. */
    tryGet<T>(token: InjectionToken<T>): T | undefined;
    /** Whether this container registers the token. */
    isRegistered(token: InjectionToken): boolean;
    /** Whether this container or an ancestor has a registration for the token. */
    isRegisteredRecursive(token: InjectionToken): boolean;
    /** Whether this container has the token registered or built. */
    has(token: InjectionToken): boolean;
    /** Whether `has` holds here or in an ancestor. */
    hasRecursive(token: InjectionToken): boolean;
    /** Keys of the tokens registered directly on this container. */
    getRegisteredTokens(): symbol[];
    /** Replaces the single registration for a token. */
    replace<T, TData = void>(token: InjectionToken<T>, factory: ServiceFactory<T, TData> | ServiceLifecycle<T, TData>, lifetime: ServiceLifetime, options?: ReplacementOptions): Promise<BindingHandle<T, TData>>;
    /** Replaces a binding by id. */
    replaceBinding<T, TData = void>(bindingId: string, factory: ServiceFactory<T, TData> | ServiceLifecycle<T, TData>, lifetime?: ServiceLifetime, options?: ReplacementOptions): Promise<BindingHandle<T, TData>>;
    /** Removes the registration for a token. */
    unregister(token: InjectionToken): void | Promise<void>;
    /** Removes one binding by id. Returns `false` when no such binding exists. */
    unregisterBinding(bindingId: string): Promise<boolean>;
    /**
     * Disposes the instances this container built, in reverse build order, and drops them. Waits for builds in progress.
     * Instances supplied through `registerInstance` and externally owned ones are kept.
     */
    clearInstances(): Promise<void>;
    /** Initializes registrations actually owned by this scope, then emits onReady in order, once per instance. */
    initializeOwned(): Promise<void>;
    /** Disposes this container and its descendants. */
    dispose(): Promise<void>;
    /** Health of the token's built instance. Reports unhealthy when it is not built. */
    healthCheck(token: InjectionToken): Promise<HealthStatus>;
    /** Health of every built instance owned by this container. */
    healthCheckAll(): Promise<Map<symbol, HealthStatus>>;
    /** Adds an interceptor that wraps every resolution made through this container and its descendants. */
    addInterceptor(i: Interceptor): void;
    /** Removes an interceptor. Returns `false` when it was not registered here. */
    removeInterceptor(i: Interceptor): boolean;
    /** A copy of this scope's identity. */
    getScopeIdentity(): ScopeIdentity;
    /** This scope's id. */
    getScopeId(): string;
    /** This scope and its ancestors, root first, with depth and parent links. */
    getScopeChain(): ScopeTreeNode[];
    /** Counter that changes when registrations change. */
    getRegistryEpoch(): number;
    /** The registry epoch with the scope that owns it and the time it was taken. */
    epochSnapshot(): RuntimeEpochSnapshot;
    /** Descriptor of the binding a token resolves to. */
    bindingOf(token: InjectionToken): BindingDescriptor | undefined;
    /** Descriptors of every multi binding for a token, in resolution order. */
    multiBindingsOf(token: InjectionToken): MultiBindingEntry[];
    /** Lifecycle state of a token's instance. */
    instanceState(token: InjectionToken): InstanceState;
    /** Built instances owned by this container, with their descriptors. */
    cachedBindings(): CacheableBinding[];
    /** Dependency edges observed at runtime across this container and its view, without duplicates. */
    dependencyEdges(): RuntimeEdge[];
    /** Ready instances held by this container. */
    ownershipRecords(): OwnershipRecord[];
    /** Dependency edges as binding-id pairs. */
    getDependencyGraph(): {
        from: string;
        to: string;
    }[];
    /** Snapshot of bindings and their dependencies. */
    getDependencyGraphSnapshot(): RuntimeDependencyGraph;
    /** Dependency tree rooted at a token's binding. */
    getDependencyTree(token: InjectionToken): RuntimeDependencyTreeNode[];
    /** Snapshot of scopes, bindings, live instances and edges. */
    getResourceGraphSnapshot(): RuntimeResourceGraph;
    /** The declared-dependency graph of this container. */
    graph(): import('@coldmind/inject/src/core/cminject.graph.ts').DiGraph;
    /** Graph issues recorded at mutation time for this container. */
    issuesObserved(): DiIssue[];
    /** Checks the declared-dependency graph and throws `DiGraphError` if it is not sound. */
    validate(): void;
    /** Subscribes to instances whose class matches a decorator key. */
    onInstance(key: string, hook: InstanceHook): () => void;
    /** Classes visible from this container whose methods or tags match `key`, root first. */
    discover(key: string): DiscoveredClass[];
}
