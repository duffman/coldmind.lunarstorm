import { ServiceLifetime, type IContainer, type ResolutionContext, type ResolutionPathStep, type ScopeIdentity } from '@coldmind/inject/contracts';
import { lookupToken, normalizeToken, tokenName, type InjectionToken } from '@coldmind/inject/token';
import { StoredRecord, idOf, lifetimeText, lifespan, type Binding } from '@coldmind/inject/internal/binding';
import type { Build, Frame, Resolver, CompletionLink, ResolutionHost } from '@coldmind/inject/internal/host';
import { ResolutionView } from '@coldmind/inject/internal/view';
import { currentCompletion } from '@coldmind/inject/internal/construction';
import { CircularDependencyError, DiError, DiGraphError, LifecycleError, NotRegisteredError } from '@coldmind/inject/errors';
import { now, stamp, chain as then, each, guarded, promised, synchronous, errorOf, attachCause } from '@coldmind/inject/internal/async';

/** The context of a resolution that starts outside any build. Shared: it holds no per-resolution state. */
export const ROOT: Resolver = Object.freeze({ suppressInterceptors: false });
/** A context for a resolution that starts outside any build, seeing `staged` replacements when given. */
export const rootResolver = (staged?: ReadonlyMap<symbol, Binding>, completion?: CompletionLink): Resolver => {
  const current = currentCompletion(completion);
  return staged || current ? { staged, completion: current, suppressInterceptors: false } : ROOT;
};
/** The step a frame contributes to a resolution path. */
function frameStep(frame: Frame): ResolutionPathStep {
  return { bindingId: frame.binding.id, instanceId: idOf(frame.record), tokenName: tokenName(frame.binding.token), scopeId: frame.target.identity.id };
}
/** The resolution path ending at `frame`, outermost first. Built only for errors and interceptors. */
function pathOf(frame?: Frame): ResolutionPathStep[] {
  const steps: ResolutionPathStep[] = [];
  for (let f = frame; f; f = f.parent) steps.push(frameStep(f));
  return steps.reverse();
}
/** The error for a cycle closed by `binding`: the path so far, then the binding again. */
function cycleError(binding: Binding, target: { readonly identity: ScopeIdentity }, parent: Frame | undefined, cached: StoredRecord | undefined): CircularDependencyError {
  return new CircularDependencyError(tokenName(binding.token), [...pathOf(parent),
    { bindingId: binding.id, tokenName: tokenName(binding.token), scopeId: target.identity.id, instanceId: cached && idOf(cached) }]);
}
/** Whether `from` transitively waits on `to`. */
function reaches(from: Build, to: Build): boolean {
  const seen = new Set<Build>();
  const stack: Build[] = [from];
  while (stack.length) {
    const current = stack.pop()!;
    if (current === to) return true;
    if (seen.has(current)) continue;
    seen.add(current);
    if (current.waitsOn) for (const next of current.waitsOn.keys()) stack.push(next);
  }
  return false;
}
/** The build behind each pending promise, so a waiter can add a waits-for edge to it. */
const buildOf = new WeakMap<Promise<StoredRecord>, Build>();
/** Materialized objects/functions may implement initialization methods. */
const isObject = (value: unknown): value is object => value !== null && (typeof value === 'object' || typeof value === 'function');

/** One owner's explicit resolution pipeline and single-owner terminal build control. */
export class Resolution {
  /** Stable selection/routing/observation capabilities, without arbitrary registration mutation. */
  constructor(private readonly host: ResolutionHost) {}
  /** Queue new callers during replacement; already-admitted work drains without blocking itself. */
  run<T>(ctx: Resolver, work: (context: Resolver) => T | Promise<T>): T | Promise<T> {
    this.host.active();
    const waiting = this.host.waitForResolution(ctx);
    if (waiting) return waiting.then(() => this.run(ctx, work));
    if (ctx.staged) return work(ctx);
    const operation = ctx.operation ?? this.host.beginOperation();
    if (ctx.operation) operation.retain();
    try {
      const result = work(ctx.operation ? ctx : { ...ctx, operation });
      if (result instanceof Promise) return result.finally(() => operation.finish());
      operation.finish(); return result;
    } catch (error) { operation.finish(); throw error; }
  }
  /** Live records and pending promises share exact binding identity. */
  readonly records = new Map<Binding, StoredRecord>();
  /** Shared async outcomes; transient materializations are never cached here. */
  readonly pending = new Map<Binding, Promise<StoredRecord>>();
  /** Successful nontransient instances enter teardown in completion order. */
  readonly disposalOrder: Binding[] = [];
  /** Every active materialization, including transients, until a terminal transition. */
  private builds?: Set<ResolutionView>;
  /** Identity for resolution errors; no container implementation is exposed. */
  get identity(): ScopeIdentity { return this.host.identity; }
  /** Diagnostic snapshot allocated only on error/disposal paths. */
  activeBuilds(): readonly ResolutionView[] { return [...(this.builds ?? [])]; }
  /** Owner shutdown rejects callers before abort listeners can publish user outcomes. */
  cancel(): void {
    for (const view of this.activeBuilds()) this.stopBuild(view, new DiError(`Build cancelled: ${tokenName(view.binding.token)}`, 'DI_BUILD_CANCELLED'));
  }

  /** Creates a `creating` instance record for a build. Transient records are never stored, so they skip the timestamp. */
  makeRecord(binding: Binding, data: unknown): StoredRecord {
    const transient = binding.lifetime === ServiceLifetime.Transient;
    const at = transient ? 0 : stamp();
    return new StoredRecord(binding, this.host.recordOwner, data, at);
  }
  /** Runs the instance's `onInit`, from its lifecycle or its own method. Returns a promise only when the hook is async. */
  private runInit(binding: Binding, instance: unknown, container: IContainer): unknown {
    if (binding.lifecycle?.onInit) return binding.lifecycle.onInit(instance, container);
    // The callable-slot check establishes the lifecycle shape before invocation.
    if (isObject(instance) && typeof (instance as { onInit?: unknown }).onInit === 'function') return (instance as { onInit(): unknown }).onInit();
    return undefined;
  }
  /**
   * Runs the factory and `onInit` for one build: synchronously when both are synchronous, a promise as soon as either is not.
   * If the build fails after construction, the instance is disposed before the error is rethrown.
   */
  private materialize(binding: Binding, target: Resolution, data: unknown, record: StoredRecord, view: ResolutionView): StoredRecord | Promise<StoredRecord> {
    const ambient = this.host.ambient;
    let produced: unknown;
    const container: IContainer = view;
    try { produced = ambient ? ambient.run(view, () => binding.factory(container, data)) : binding.factory(container, data); }
    catch (error) { this.fail(record, view); throw record.cancelled ?? error; }
    // Only a native Promise is awaited: a user-defined thenable is the service itself.
    if (produced instanceof Promise) return this.constructAsync(binding, target, record, view, produced);
    record.instance = produced; record.produced = true;
    if (record.cancelled) { void this.cleanupLate(record); throw record.cancelled; }
    return this.initialize(binding, target, record, view, false);
  }
  /** Awaits an async factory, then initializes the instance. */
  private async constructAsync(binding: Binding, target: Resolution, record: StoredRecord, view: ResolutionView, produced: Promise<unknown>): Promise<StoredRecord> {
    try { record.instance = await produced; record.produced = true; }
    catch (error) { this.fail(record, view); throw record.cancelled ?? error; }
    if (record.cancelled) return this.cancelledResult(record);
    return this.initialize(binding, target, record, view, true);
  }
  /** Runs `onInit`, then marks the build ready. `async` says whether the build already went asynchronous. */
  private initialize(binding: Binding, target: Resolution, record: StoredRecord, view: ResolutionView, async: boolean): StoredRecord | Promise<StoredRecord> {
    if (record.cancelled) return this.cancelledResult(record);
    record.state = 'initializing';
    const ambient = this.host.ambient;
    let init: unknown;
    const container: IContainer = view;
    try { init = ambient ? ambient.run(view, () => this.runInit(binding, record.instance, container)) : this.runInit(binding, record.instance, container); }
    catch (error) { return record.cancelled ? this.cancelledResult(record) : this.discard(record, view, error); }
    if (init instanceof Promise) return init.then(
      () => record.cancelled ? this.cancelledResult(record) : this.ready(binding, target, record, view, true),
      error => record.cancelled ? this.cancelledResult(record) : this.discard(record, view, error));
    return record.cancelled ? this.cancelledResult(record) : this.ready(binding, target, record, view, async);
  }
  /**
   * Marks a build ready, queues a stored instance for disposal, and ends the build's view. A build that finished
   * synchronously keeps its creation time as its last change: no time passed in between.
   */
  private ready(binding: Binding, target: Resolution, record: StoredRecord, view: ResolutionView, async: boolean): StoredRecord {
    record.state = 'ready';
    if (binding.lifetime !== ServiceLifetime.Transient) { if (async) record.lastTouchedAt = stamp(); target.disposalOrder.push(binding); }
    view.done = true;
    return record;
  }
  /** Marks a build failed and ends its view. */
  private fail(record: StoredRecord, view: ResolutionView): void {
    record.state = 'failed';
    if (record.binding.lifetime !== ServiceLifetime.Transient) record.lastTouchedAt = stamp();
    view.done = true;
  }
  /** Fails a constructed build: disposes the instance, then rethrows `error`, with any cleanup failure as its cause. */
  private discard(record: StoredRecord, view: ResolutionView, error: unknown): Promise<never> {
    this.fail(record, view);
    let cleanup: unknown;
    try { cleanup = this.host.dispose(record, true); }
    catch (cleanupError) { attachCause(error, cleanupError); throw error; }
    if (cleanup instanceof Promise) return cleanup.then(() => { throw error; }, cleanupError => { attachCause(error, cleanupError); throw error; });
    throw error;
  }
  /**
   * Returns the record for a binding, building it if needed: synchronously when every step is synchronous, a promise otherwise.
   * A cycle is detected from the frames of this resolution, and across concurrent resolutions from the waits-for graph.
   */
  entry(binding: Binding, data: unknown, ctx: Resolver): StoredRecord | Promise<StoredRecord> {
    // Internal initialization/replacement loops also enter here after awaits: shutdown must stop new builds.
    this.host.active();
    // A tenant or request service built while a longer-lived one is being built would outlive its scope.
    const span = lifespan(binding.lifetime);
    if (span && span < 3) {
      for (let f = ctx.frame; f; f = f.parent) {
        if (lifespan(f.binding.lifetime) <= span) continue;
        throw new DiGraphError(this.host.exposeIssues([{ kind: 'captive', at: binding.key,
          message: `${tokenName(binding.token)} is ${lifetimeText[binding.lifetime]}-scoped and is being resolved while ${tokenName(f.binding.token)} (${lifetimeText[f.binding.lifetime]}) is being built. The instance would be kept past its scope.` }]));
      }
    }
    const target = this.host.route(binding);
    if (target !== this) target.host.active();
    const parent = ctx.frame;
    const transient = binding.lifetime === ServiceLifetime.Transient;
    if (parent) this.host.observe(parent.binding, binding);
    const cached = transient ? undefined : target.records.get(binding);
    // A built or building instance is shared: data that differs from the data it was built with would be silently ignored.
    if (cached && data !== undefined && cached.data !== undefined && !Object.is(cached.data, data)) {
      throw new LifecycleError(`${tokenName(binding.token)} is already built with other data; a ${lifetimeText[binding.lifetime]} service is built once per scope`);
    }
    // Same chain: this binding is already being built further up it.
    for (let f = parent; f; f = f.parent) if (f.binding === binding) throw cycleError(binding, target, parent, cached);
    if (binding.lifetime === ServiceLifetime.Scoped) {
      for (let f = parent; f; f = f.parent) {
        if (f.binding.lifetime !== ServiceLifetime.Singleton) continue;
        throw new DiGraphError(this.host.exposeIssues([{ kind: 'captive', at: binding.key,
          message: `${tokenName(binding.token)} is scoped and is being resolved while a singleton is being built. The scoped instance would be cached on the singleton's container and shared by every caller.` }]));
      }
    }
    if (cached?.state === 'ready') return this.linked(parent, cached);
    if (!transient) {
      // A pending build always has its record stored first, so without a record there is nothing to look up.
      const pending = cached === undefined ? undefined : target.pending.get(binding);
      if (pending) {
        const awaited = buildOf.get(pending)!;
        // Cross-chain cycle: the build we wait on already waits, directly or not, on the build asking for it.
        if (ctx.build && reaches(awaited, ctx.build)) throw cycleError(binding, target, parent, cached);
        return this.settled(ctx, awaited, pending, parent);
      }
      // Built synchronously further up the call stack by another resolution: waiting would never end.
      if (cached && (cached.state === 'creating' || cached.state === 'initializing')) throw cycleError(binding, target, parent, cached);
    }
    const record = target.makeRecord(binding, data);
    const timeout = binding.timeoutMs === undefined ? target.host.buildTimeout : binding.timeoutMs;
    record.deadline = typeof timeout === 'number' ? now() + timeout : undefined;
    const view = target.host.view(binding, record, parent, ctx);
    (target.builds ??= new Set()).add(view);
    if (!transient) target.records.set(binding, record);
    let out: StoredRecord | Promise<StoredRecord>;
    try { out = this.materialize(binding, target, data, record, view); }
    catch (error) {
      target.finishBuild(view);
      if (!transient && !record.cancelled) target.share(binding, Promise.reject(error), view);
      throw error;
    }
    if (!(out instanceof Promise)) { target.finishBuild(view); return this.linked(parent, out); }
    const controlled = target.watchBuild(view, out, timeout);
    if (!transient && !record.cancelled) target.share(binding, controlled, view);
    return this.settled(ctx, view, controlled, parent);
  }
  /** Ends tracking and removes timer/waits state without touching another retry's cache slot. */
  private finishBuild(view: ResolutionView): void {
    const record = view.record;
    record.building = false; record.rejectBuild = undefined;
    if (record.timer !== undefined) { clearTimeout(record.timer); record.timer = undefined; }
    this.builds?.delete(view); view.waitsOn?.clear(); view.done = true; view.parent = undefined;
  }
  /** Consumes late failures and disposes only owned values never returned to a caller. */
  private async cleanupLate(record: StoredRecord): Promise<void> {
    if (!record.produced) return;
    try { const out = this.host.dispose(record, true); if (out instanceof Promise) await out; }
    catch (error) { if (record.cancelled) attachCause(record.cancelled, error); }
  }
  /** Keeps late materialization cleanup in the consumed raw promise, not in a caller's cancellation path. */
  private async cancelledResult(record: StoredRecord): Promise<never> { await this.cleanupLate(record); throw record.cancelled; }
  /** Rejects current callers before user abort handlers can settle their factory promises. */
  private stopBuild(view: ResolutionView, error: DiError): void {
    const record = view.record;
    if (!record.building) return;
    record.cancelled = error; record.state = 'failed';
    const reject = record.rejectBuild;
    this.finishBuild(view);
    if (this.records.get(view.binding) === record) {
      this.records.delete(view.binding);
      const index = this.disposalOrder.indexOf(view.binding); if (index !== -1) this.disposalOrder.splice(index, 1);
    }
    const pending = this.pending.get(view.binding);
    if (pending && buildOf.get(pending) === view) this.pending.delete(view.binding);
    reject?.(error); record.abort(error);
  }
  /** Names the pending materializations and live waits before timeout tears them down. */
  private timeoutError(view: ResolutionView, milliseconds: number): DiError {
    const pending = this.host.allBuilds();
    const names = pending.map(item => `${tokenName(item.binding.token)}@${item.target.identity.id}`);
    const edges = pending.flatMap(item => [...(item.waitsOn?.keys() ?? [])].map(other => `${tokenName(item.binding.token)} -> ${tokenName(other.binding.token)}`));
    return new DiError(`Build timeout ${tokenName(view.binding.token)} after ${milliseconds}ms; pending: ${names.join(', ')}; waits-for: ${edges.join(', ') || 'none'}; path: ${pathOf(view).map(step => step.tokenName).join(' -> ')}`, 'DI_BUILD_TIMEOUT');
  }
  /** Publishes the same cancellation/deadline boundary to every kind of caller. */
  private watchBuild(view: ResolutionView, raw: Promise<StoredRecord>, timeout: number | null | undefined): Promise<StoredRecord> {
    const record = view.record;
    return new Promise<StoredRecord>((resolve, reject) => {
      if (record.cancelled) reject(record.cancelled); else record.rejectBuild = reject;
      raw.then(value => {
        if (record.cancelled) { void this.cleanupLate(record); return; }
        this.finishBuild(view); resolve(value);
      }, error => { this.finishBuild(view); reject(record.cancelled ?? error); });
      if (record.building && typeof timeout === 'number') {
        // Native timers clamp delays above signed 32-bit range to about 1ms. Segment rather
        // than narrowing the public finite-number contract; every segment checks monotonic time.
        const arm = (): void => {
          record.timer = setTimeout(() => {
            if (!record.building) return;
            if (record.deadline! > now()) arm();
            else this.stopBuild(view, this.timeoutError(view, timeout));
          }, Math.min(2147483647, Math.max(0, record.deadline! - now())));
          record.timer.unref?.();
        };
        arm();
      }
    });
  }
  /** Logs the resource edge from the asking build, if any, and returns the record. */
  private linked(parent: Frame | undefined, record: StoredRecord): StoredRecord {
    if (parent) this.host.observeResource(parent.record, record);
    return record;
  }
  /** Awaits a build on behalf of `ctx`, holding a waits-for edge from the asking build while it runs. */
  private settled(ctx: Resolver, awaited: Build | undefined, promise: Promise<StoredRecord>, parent: Frame | undefined): Promise<StoredRecord> {
    const from = ctx.build;
    if (!from || !awaited || awaited === from) return promise.then(record => this.linked(parent, record));
    const waitsOn = from.waitsOn ??= new Map();
    waitsOn.set(awaited, (waitsOn.get(awaited) ?? 0) + 1);
    const release = () => {
      const count = waitsOn.get(awaited);
      if (count === undefined) return;
      if (count > 1) waitsOn.set(awaited, count - 1); else waitsOn.delete(awaited);
    };
    return promise.then(record => { release(); return this.linked(parent, record); }, error => { release(); throw error; });
  }
  /** Publishes a build as pending, so concurrent resolves share it, and clears the slot once it settles. */
  private share(id: Binding, promise: Promise<StoredRecord>, build: Build): void {
    this.pending.set(id, promise);
    buildOf.set(promise, build);
    const clear = () => { if (this.pending.get(id) === promise) this.pending.delete(id); };
    promise.then(clear, clear);
  }

  /** Resolves a single visible binding. Interceptors run on cache hits too. */
  resolve<T, D = void>(token: InjectionToken<T>, data?: D): Promise<T> { return promised<T>(() => this.resolveIn(this.rootCtx(), token, data)); }
  /** The context a resolve made outside any build starts in: the running factory's, when an ambient source is installed and one runs. */
  rootCtx(): Resolver {
    const current = this.host.ambient?.current();
    return current instanceof ResolutionView && !current.done && current.record.container.root === this.host.root ? current : ROOT;
  }
  /** Resolves a single token: its value when every step is synchronous, otherwise a promise of it. Used to build classes. */
  resolveEither(token: InjectionToken): unknown {
    return then(this.resolveIn(this.rootCtx(), token, undefined), record => record.instance);
  }
  /**
   * Resolves a single binding synchronously. Throws `LifecycleError` when any step is asynchronous: an async factory or
   * `onInit`, an async interceptor, or a build already in progress. An async build it started keeps running and is
   * cached when done, so a later `get`, `resolve` or `resolveSync` finds it.
   */
  resolveSync<T, D = void>(token: InjectionToken<T>, data?: D): T { return synchronous<T>(this.resolveIn(this.rootCtx(), token, data), token); }

  /**
   * Resolves one single token within a resolution context: a record when every step is synchronous, otherwise a promise.
   * `known` is the token's key when the caller already has it.
   */
  resolveIn(ctx: Resolver, token: InjectionToken, data: unknown, known?: symbol): StoredRecord | Promise<StoredRecord> {
    return this.run(ctx, context => this.resolveSelected(context, token, data, known));
  }
  private resolveSelected(ctx: Resolver, token: InjectionToken, data: unknown, known?: symbol): StoredRecord | Promise<StoredRecord> {
    this.host.active();
    const key = known ?? lookupToken(token) ?? Symbol(tokenName(token));
    if (!ctx.suppressInterceptors && this.host.hasInterceptors()) return this.resolveIntercepted(ctx, token, normalizeToken(token), data);
    const binding = this.host.selectSingle(key, ctx.staged, ctx) ?? this.host.autoBinding(token, key);
    if (!binding) throw new NotRegisteredError(token, tokenName(token));
    this.host.assertSingle(key, binding, token);
    return this.entry(binding, data, ctx);
  }
  /** `resolveIn` with interceptors: before, then the resolution, then after; on failure, the error hooks. */
  private resolveIntercepted(ctx: Resolver, token: InjectionToken, key: symbol, data: unknown): StoredRecord | Promise<StoredRecord> {
    const interceptors = this.host.interceptors();
    const binding = this.host.selectSingle(key, ctx.staged, ctx) ?? this.host.autoBinding(token, key);
    const path = pathOf(ctx.frame);
    const scopeId = binding && this.host.scopeId(binding);
    const context: ResolutionContext = {
      token: key, lifetime: binding?.lifetime ?? ServiceLifetime.Transient, data, attempt: 1, requester: path.at(-1),
      path: binding ? [...path, { bindingId: binding.id, tokenName: tokenName(binding.token), scopeId: scopeId ?? this.host.identity.id }] : path,
    };
    const start = now();
    const fail = (error: unknown): Promise<never> => {
      const original = errorOf(error);
      const hooks = each(interceptors, interceptor => guarded(error, () => interceptor.onResolutionError?.(context, original)));
      if (hooks instanceof Promise) return hooks.then(() => { throw error; });
      throw error;
    };
    try {
      const out = then(each(interceptors, interceptor => interceptor.onBeforeResolve?.(context)), () => {
        if (!binding) throw new NotRegisteredError(token, tokenName(token));
        this.host.assertSingle(key, binding, token);
        return then(this.entry(binding, data, ctx), record =>
          then(each(interceptors, interceptor => interceptor.onAfterResolve?.(context, record.instance, now() - start)), () => record));
      });
      return out instanceof Promise ? out.catch(fail) : out;
    } catch (error) { return fail(error); }
  }

  /** Multi entries resolve sequentially (ancestor registrations first). */
  resolveAll<T, D = void>(token: InjectionToken<T>, data?: D): Promise<T[]> { return this.resolveAllIn<T>(this.rootCtx(), token, data); }

  /** Resolves every multi entry for a token, in order, within a resolution context. */
  async resolveAllIn<T>(ctx: Resolver, token: InjectionToken, data: unknown): Promise<T[]> {
    return this.run(ctx, context => this.resolveAllSelected<T>(context, token, data));
  }
  private async resolveAllSelected<T>(ctx: Resolver, token: InjectionToken, data: unknown): Promise<T[]> {
    this.host.active();
    let key = lookupToken(token) ?? Symbol(tokenName(token));
    const binding = this.host.selectSingle(key, ctx.staged, ctx);
    const entries = this.host.selectMulti(key, ctx.staged);
    const interceptors = ctx.suppressInterceptors || !this.host.hasInterceptors() ? [] : this.host.interceptors();
    if (interceptors.length) key = normalizeToken(token);
    const path = interceptors.length ? pathOf(ctx.frame) : [];
    const context: ResolutionContext<T[]> = { token: key, data, attempt: 1,
      lifetime: entries[0]?.lifetime ?? binding?.lifetime ?? ServiceLifetime.Transient,
      path, requester: path.at(-1) };
    const start = interceptors.length ? now() : 0;
    for (const interceptor of interceptors) await interceptor.onBeforeResolve?.(context);
    try {
      if (!entries.length) {
        // The selected binding belongs to the requested InjectionToken<T>.
        const single = binding ? [(await this.entry(binding, data, ctx)).instance as T] : [];
        for (const interceptor of interceptors) await interceptor.onAfterResolve?.(context, single, now() - start);
        return single;
      }
      // A multi entry already in this chain is a cycle; check all entries before building any.
      for (const entry of entries) {
        for (let f = ctx.frame; f; f = f.parent) {
          if (f.binding !== entry) continue;
          throw new CircularDependencyError(tokenName(entry.token), [...pathOf(ctx.frame),
            { bindingId: entry.id, tokenName: tokenName(entry.token), scopeId: this.host.identity.id }]);
        }
      }
      // Each caller resolves its own entries. Entries share builds through the pending slot, not a shared batch.
      const results: T[] = [];
      // All selected multi entries belong to the requested InjectionToken<T>.
      for (const entry of entries) results.push((await this.entry(entry, data, ctx)).instance as T);
      for (const interceptor of interceptors) await interceptor.onAfterResolve?.(context, results, now() - start);
      return results;
    } catch (error) {
      for (const interceptor of interceptors) {
        try { await interceptor.onResolutionError?.(context, errorOf(error)); }
        catch (hookError) { attachCause(error, hookError); }
      }
      throw error;
    }
  }
}
