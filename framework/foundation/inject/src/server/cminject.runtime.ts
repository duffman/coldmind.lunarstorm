import { AsyncLocalStorage } from 'node:async_hooks';
import { Container } from '@coldmind/inject/container';
import { runInContext } from '@coldmind/inject/internal/server/context';
import { type IContainer, ScopeType } from '@coldmind/inject/contracts';
import { DiError } from '@coldmind/inject/errors';

/** Lifecycle state of a `DiRuntime`. */
export type RuntimeState = 'created' | 'starting' | 'ready' | 'draining' | 'failed' | 'disposed';
/** Lifecycle state of a tenant scope under a runtime. */
export type TenantState = 'creating' | 'ready' | 'draining' | 'failed' | 'disposed';
/** Where in the runtime lifecycle an error occurred. */
export type RuntimeErrorPhase = 'start' | 'configure-tenant' | 'validate' | 'initialize' | 'acquire' | 'release' | 'drain-tenant' | 'dispose';
/** Stable code of a runtime error. */
export type RuntimeErrorCode = 'RUNTIME_NOT_READY' | 'RUNTIME_DRAINING' | 'RUNTIME_DISPOSED' | 'RUNTIME_FAILED'
  | 'TENANT_DRAINING' | 'TENANT_FAILED' | 'INVALID_SCOPE';
/** Context passed to `onError`. */
export interface RuntimeErrorContext<TId> {
  /** The lifecycle phase that failed. */
  phase: RuntimeErrorPhase;
  /** The tenant involved, if any. */
  tenantId?: TId;
  /** The kind of scope involved, if any. */
  scopeType?: ScopeType;
}
/** Options for `DiRuntime`. */
export interface DiRuntimeOptions<TId> {
  /** Configures a tenant scope the first time it is created. */
  configureTenant?(tenantId: TId, container: IContainer): void | Promise<void>;
  /** Called for every runtime error. It cannot break cleanup. */
  onError?(error: unknown, context: RuntimeErrorContext<TId>): void;
  /** Maps a tenant id to a scope id. */
  tenantScopeId?(tenantId: TId): string;
  /** Build a request scope's own registrations when it is acquired. Off by default: services build on first resolve. */
  eagerRequest?: boolean;
}
/** A request scope held by the caller until `release`. */
export interface RequestScopeLease {
  /** The request scope. */
  container: IContainer;
  /** Runs work within this request scope. */
  run<T>(work: (c: IContainer) => T | Promise<T>): Promise<T>;
  /** Disposes the request scope and releases its tenant lease. */
  release(): Promise<void>;
}
/** Thrown by `DiRuntime` for lifecycle and tenant errors. `code` identifies the error. */
export class DiRuntimeError extends DiError {
  /** Creates a runtime error with a stable `code`. */
  constructor(message: string, code: RuntimeErrorCode, options?: ErrorOptions) { super(message, code, options); }
}
/** A tenant scope with its state and the leases on it. */
interface TenantEntry<TId> {
  /** The tenant id. */
  id: TId;
  /** The tenant scope. */
  scope: Container;
  /** Lifecycle state of the tenant. */
  state: TenantState;
  /** Why initialization failed. */
  failure?: unknown;
  /** Settles when `configureTenant`, validation and initialization are done. */
  initializing: Promise<void>;
  /** The drain in progress, once started. */
  draining?: Promise<void>;
  /** Number of active leases. Drain waits for zero. */
  leases: number;
  /** Callbacks run when the lease count reaches zero. */
  idle: Array<() => void>;
}

/** Tenant/request lifecycle manager with ref-counted leases and graceful draining. */
export class DiRuntime<TId> {
  /** Build context belongs to this runtime, independently of public request context. */
  private readonly resolutions = new AsyncLocalStorage<object>();
  /** The root container. */
  readonly root: IContainer;
  /** The root container with its internal API. */
  private readonly rootContainer: Container;
  /** Options given to the constructor. */
  private readonly options: DiRuntimeOptions<TId>;
  /** Tenant entries, by tenant id. */
  private readonly tenants = new Map<TId, TenantEntry<TId>>();
  /** Scope ids handed out for object tenant ids. */
  private readonly refs = new WeakMap<object, string>();
  /** Counter for object tenant scope ids. */
  private nextRef = 0;
  /** Lifecycle state of the runtime. */
  private runtimeState: RuntimeState = 'created';
  /** The start in progress. */
  private starting?: Promise<void>;
  /** The disposal in progress or done. */
  private disposing?: Promise<void>;
  /** Creates a runtime over `root`, or over a new root container. */
  constructor(root: Container = new Container(), options: DiRuntimeOptions<TId> = {}) {
    this.rootContainer = root; this.root = root; this.options = options;
    const storage = this.resolutions;
    root.useAmbient({ run: (frame, work) => storage.run(frame, work), current: () => storage.getStore() });
  }
  /** Current lifecycle state of the runtime. */
  get state(): RuntimeState { return this.runtimeState; }
  /** Current state of a tenant, or `undefined` when it has no scope. */
  tenantState(id: TId): TenantState | undefined { return this.tenants.get(id)?.state; }
  /** Passes an error to `onError`. Errors thrown by the callback are swallowed so they cannot break cleanup. */
  private report(error: unknown, phase: RuntimeErrorPhase, tenantId?: TId): void {
    try { this.options.onError?.(error, { phase, ...(tenantId === undefined ? {} : { tenantId }) }); } catch { /* user errors never break cleanup */ }
  }
  /** Throws the error matching the current runtime state. */
  private failedByState(): never {
    const state = this.runtimeState;
    const code: RuntimeErrorCode = state === 'disposed' ? 'RUNTIME_DISPOSED' : state === 'draining' ? 'RUNTIME_DRAINING'
      : state === 'failed' ? 'RUNTIME_FAILED' : 'RUNTIME_NOT_READY';
    throw new DiRuntimeError(`Runtime is ${state}`, code);
  }
  /** Throws unless the runtime is `ready`. */
  private requireReady(): void { if (this.runtimeState !== 'ready') this.failedByState(); }
  /** Validates the root graph and moves the runtime to `ready`. Fails the runtime when validation fails. */
  async start(): Promise<void> {
    if (this.runtimeState === 'ready') return;
    if (this.runtimeState === 'disposed' || this.runtimeState === 'draining') this.failedByState();
    if (this.starting) return this.starting;
    this.runtimeState = 'starting';
    const before = new Set(this.rootContainer.cachedBindings().map(b => b.id));
    this.starting = (async () => {
      try {
        this.rootContainer.validate();
        await this.rootContainer.initializeOwned();
        this.runtimeState = 'ready';
      } catch (error) {
        this.report(error, 'start');
        await this.rootContainer.rollbackNewInstances(before);
        this.runtimeState = 'failed';
        throw new DiRuntimeError('Runtime failed to start', 'RUNTIME_FAILED', { cause: error });
      }
    })();
    try { await this.starting; } finally { this.starting = undefined; }
  }
  /** Scope id for a tenant: from `tenantScopeId` when given, otherwise derived from the tenant id. */
  private scopeId(id: TId): string {
    if (this.options.tenantScopeId) return this.options.tenantScopeId(id);
    if (typeof id === 'string') return id;
    if (typeof id === 'number' || typeof id === 'bigint' || typeof id === 'boolean') return String(id);
    if (typeof id === 'symbol') return `symbol:${id.description ?? ''}`;
    if ((typeof id === 'object' && id !== null) || typeof id === 'function') {
      const key = id as object;
      let result = this.refs.get(key);
      if (!result) { result = `tenant:${++this.nextRef}`; this.refs.set(key, result); }
      return result;
    }
    return String(id);
  }
  /** Returns the tenant entry, creating its scope and running `configureTenant` on first use. */
  private async createOrGetTenant(id: TId): Promise<TenantEntry<TId>> {
    this.requireReady();
    let entry = this.tenants.get(id);
    if (!entry) {
      const scope = this.rootContainer.createTenantScope(this.scopeId(id));
      entry = { id, scope, state: 'creating', initializing: Promise.resolve(), leases: 0, idle: [] };
      this.tenants.set(id, entry);
      const current = entry;
      entry.initializing = (async () => {
        /** Lifecycle stage whose failure is reported to the observer. */
        let phase: RuntimeErrorPhase = 'configure-tenant';
        try {
          await this.options.configureTenant?.(id, scope);
          phase = 'validate'; scope.validate();
          phase = 'initialize'; await scope.initializeOwned();
          if (current.state === 'creating') current.state = 'ready';
        } catch (error) {
          this.report(error, phase, id);
          current.failure = error;
          if (current.state !== 'draining') current.state = 'failed';
          await this.disposeScope(scope, 'dispose', id);
          throw new DiRuntimeError('Tenant initialization failed', 'TENANT_FAILED', { cause: error });
        }
      })();
    }
    if (entry.state === 'failed') {
      await entry.initializing;
      throw new DiRuntimeError('Tenant initialization failed', 'TENANT_FAILED', { cause: entry.failure });
    }
    if (entry.state === 'draining' || entry.state === 'disposed') throw new DiRuntimeError('Tenant is draining', 'TENANT_DRAINING');
    await entry.initializing;
    if (entry.state !== 'ready') {
      if (entry.failure) throw new DiRuntimeError('Tenant initialization failed', 'TENANT_FAILED', { cause: entry.failure });
      throw new DiRuntimeError('Tenant is draining', 'TENANT_DRAINING');
    }
    return entry;
  }
  /** Takes a lease on a tenant, so drain waits for it. Returns the release function. */
  private async lease(id: TId): Promise<{ entry: TenantEntry<TId>; release: () => void }> {
    const entry = await this.createOrGetTenant(id);
    this.requireReady();
    if (entry.state !== 'ready') throw new DiRuntimeError('Tenant is draining', 'TENANT_DRAINING');
    entry.leases++;
    let done = false;
    return { entry, release: () => {
      if (done) return;
      done = true; entry.leases--;
      if (entry.leases === 0) for (const complete of entry.idle.splice(0)) complete();
    } };
  }
  /** Resolves once a tenant has no leases. */
  private waitIdle(entry: TenantEntry<TId>): Promise<void> {
    return entry.leases ? new Promise(resolve => entry.idle.push(resolve)) : Promise.resolve();
  }
  /** Disposes a scope and reports any failure through `onError`. */
  private async disposeScope(container: Container, phase: RuntimeErrorPhase, tenantId?: TId): Promise<void> {
    const tracked = await container.disposeWithTracking();
    for (const failure of tracked) if (failure.error) this.report(failure.error, phase, tenantId);
  }
  /** Runs work within a tenant scope, creating it on first use. The tenant is leased for the duration. */
  async withTenant<T>(id: TId, work: (c: IContainer) => T | Promise<T>): Promise<T> {
    const lease = await this.lease(id);
    try { return await runInContext(lease.entry.scope, () => work(lease.entry.scope)); }
    finally { lease.release(); }
  }
  /** Creates a request scope under a tenant. The caller must call `release` on the returned lease. */
  async acquireRequest(id: TId): Promise<RequestScopeLease> {
    const lease = await this.lease(id);
    const scope = lease.entry.scope.createRequestScope();
    try { if (this.options.eagerRequest) await scope.initializeOwned(); }
    catch (error) {
      this.report(error, 'acquire', id);
      await this.disposeScope(scope, 'release', id);
      lease.release(); throw error;
    }
    let releasePromise: Promise<void> | undefined;
    return {
      container: scope,
      run: async <T>(work: (c: IContainer) => T | Promise<T>): Promise<T> => {
        if (releasePromise) throw new DiRuntimeError('Request scope has been released', 'INVALID_SCOPE');
        return runInContext(scope, () => Promise.resolve(work(scope)));
      },
      release: () => releasePromise ??= (async () => {
        try { await this.disposeScope(scope, 'release', id); }
        finally { lease.release(); }
      })(),
    };
  }
  /** Runs work within a new request scope, releasing it afterwards. */
  async withRequest<T>(id: TId, work: (c: IContainer) => T | Promise<T>): Promise<T> {
    const lease = await this.acquireRequest(id);
    try { return await lease.run(work); } finally { await lease.release(); }
  }
  /** Drains one tenant entry, moving it through `draining` to `disposed`. */
  private drainEntry(entry: TenantEntry<TId>, phase: RuntimeErrorPhase): Promise<void> {
    if (entry.draining) return entry.draining;
    entry.state = 'draining';
    entry.draining = (async () => {
      try { await entry.initializing; } catch { /* failure already recorded */ }
      await this.waitIdle(entry);
      await this.disposeScope(entry.scope, phase, entry.id);
      entry.state = 'disposed';
      this.tenants.delete(entry.id);
    })();
    return entry.draining;
  }
  /** Waits for a tenant's leases to end, then disposes its scope. */
  async drainTenant(id: TId): Promise<void> {
    const entry = this.tenants.get(id);
    if (entry) await this.drainEntry(entry, 'drain-tenant');
  }
  /** Stops accepting new leases and drains every tenant. */
  async drain(): Promise<void> {
    if (this.runtimeState === 'created' || this.runtimeState === 'failed' || this.runtimeState === 'disposed') return;
    if (this.starting) { try { await this.starting; } catch { return; } }
    this.runtimeState = 'draining';
    for (const entry of [...this.tenants.values()]) {
      try { await entry.initializing; } catch { /* no work can be leased */ }
    }
    await Promise.all([...this.tenants.values()].map(e => this.waitIdle(e)));
  }
  /** Drains every tenant and disposes the runtime. Terminal. */
  async dispose(): Promise<void> {
    if (this.disposing) return this.disposing;
    if (this.runtimeState === 'disposed') return;
    this.disposing = (async () => {
      if (this.starting) { try { await this.starting; } catch { /* continue cleanup */ } }
      if (this.runtimeState !== 'failed') this.runtimeState = 'draining';
      for (const entry of [...this.tenants.values()]) await this.drainEntry(entry, 'dispose');
      try { await this.disposeScope(this.rootContainer, 'dispose'); }
      finally { this.resolutions.disable(); this.runtimeState = 'disposed'; }
    })();
    return this.disposing;
  }
}
