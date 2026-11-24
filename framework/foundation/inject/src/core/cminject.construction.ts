import { isDeferredToken, unwrapDependencySlot, bindDeferredSlot, type DependencySlot } from '@coldmind/inject/lazy';
import type { IContainer, ClassDescriptor, ParameterInjection, ServiceFactory } from '@coldmind/inject/contracts';
import type { Constructor, InjectionToken } from '@coldmind/inject/token';
import type { BindingDependency } from '@coldmind/inject/internal/binding';
import type { CompletionState, CompletionAttempt, CompletionLink, Scope, ConstructionHost } from '@coldmind/inject/internal/host';
import { DiError, NotRegisteredError, LifecycleError } from '@coldmind/inject/errors';
import { describe } from '@coldmind/inject/decorators';
import { chain as then, each, gather, shield } from '@coldmind/inject/internal/async';

/** Constructor arguments of a build that supplies none. */
const NO_ARGS: readonly unknown[] = Object.freeze([]);
/** Skips expired hooks and retries without losing a still-active outer hook. */
export function currentCompletion(link?: CompletionLink): CompletionLink | undefined {
  for (let item = link; item; item = item.parent) {
    if (item.valid && item.attempt.active && item.attempt.state.attempt === item.attempt) return item;
  }
  return undefined;
}
/** Original method and generated wrappers, shared weakly so separate roots do not wrap another root's wrapper. */
interface MethodOrigin {
  /** The actual method before container wrapping. */
  original: (...args: unknown[]) => unknown;
  /** Generated wrappers recognized without retaining old roots' functions. */
  generated: WeakSet<Function>;
}
/** Method provenance does not own instances or skip any tree's completion. */
const methodOrigins = new WeakMap<object, Map<string | symbol, MethodOrigin>>();
/** Declared dependencies of a class: its injected parameters, then its injected members. */
export function classEdges(descriptor: ClassDescriptor, params: readonly ParameterInjection[]): BindingDependency[] {
  return [
    ...params.filter(p => p.token !== undefined).map(p => ({ token: unwrapDependencySlot(p.token!), lazy: isDeferredToken(p.token), index: p.index, optional: p.optional, multi: p.multi })),
    ...descriptor.members.map((m, i) => ({ token: unwrapDependencySlot(m.token), lazy: isDeferredToken(m.token), index: params.length + i,
      property: m.name, optional: m.optional, multi: m.multi })),
  ];
}
/** Supplied objects and functions can carry injected fields and methods. */
const isObject = (value: unknown): value is object => value !== null && (typeof value === 'object' || typeof value === 'function');

/** Constructor injection and root-owned completion; its host deliberately has no registry mutation capabilities. */
export class Construction {
  /** Contextual read/completion capabilities bound once by the owning scope. */
  constructor(private readonly host: ConstructionHost) {}
  /** Root progress exists only after the first completion, and never owns supplied instances strongly. */
  private completionStates?: WeakMap<object, CompletionState>;
  /** Root identity and progress shared by descendants through the narrow host. */
  get states(): WeakMap<object, CompletionState> { return this.completionStates ??= new WeakMap(); }
  /** Factory that constructs `ctor` with `params`, then applies the descriptor's members, wraps and instance hooks. Synchronous unless a dependency is asynchronous. */
  factory<T, D = void>(ctor: Constructor<T>, descriptor: ClassDescriptor, params: readonly ParameterInjection[]): ServiceFactory<T, D> {
    if (!params.length && !descriptor.members.length && !descriptor.wraps.length && !descriptor.methods.length && !descriptor.tags.length) {
      return (_scope: IContainer, _data?: unknown) => new ctor();
    }
    return (scope: IContainer, _data?: unknown) => {
      return this.construct(this.host.scope(scope), ctor, descriptor, params, NO_ARGS);
    };
  }
  /** Resolves an authored factory tuple without forcing synchronous dependencies async. */
  dependencies(container: IContainer, slots: readonly DependencySlot[]): unknown[] | Promise<unknown[]> {
    const scope = this.host.scope(container);
    return gather(slots.length, index => this.slot(scope, slots[index], false, false));
  }
  /**
   * Builds `ctor`. `lead` fills the first constructor parameters; the container resolves the rest from `params`; then the
   * instance is completed. Returns the instance when every step is synchronous, otherwise a promise of it.
   */
  private construct<T>(scope: Scope, ctor: Constructor<T>, descriptor: ClassDescriptor, params: readonly ParameterInjection[], lead: readonly unknown[]): T | Promise<T> {
    const rest = params.filter(p => p.index >= lead.length);
    for (const p of rest) {
      if (!p.token && !p.optional) {
        throw new DiError(`Cannot resolve dependency at index ${p.index} for ${ctor.name}. Name it in \`static $inject\`, the class decorator's \`deps\`, or (legacy decorators) @Inject() on the parameter. A parameter typed as an interface, union or primitive has no class to resolve by.`, 'DI_UNRESOLVABLE_PARAMETER');
      }
    }
    // Reflect.construct returns the instance of this concrete constructor; injection supplies its validated argument slots.
    return then(gather(rest.length, i => this.slot(scope, rest[i]!.token, rest[i]!.optional, rest[i]!.multi)),
      args => this.complete(Reflect.construct(ctor, [...lead, ...args]) as T, ctor, descriptor, scope));
  }
  /** Resolves one injected slot: a value when synchronous, otherwise a promise. An optional slot with no registration is `undefined`. */
  private slot(scope: Scope, token: DependencySlot | undefined, optional: boolean, multi: boolean): unknown {
    if (!token) return undefined;
    if (isDeferredToken(token)) {
      const deferredScope = {
        resolve: <T>(value: InjectionToken<T>): Promise<T> => Promise.resolve(scope.resolveEither(value) as T),
        resolveSync: <T>(value: InjectionToken<T>): T => {
          const out = scope.resolveEither(value);
          if (out instanceof Promise) { void out.catch(() => undefined); throw new LifecycleError('Deferred getSync requires synchronous construction'); }
          return out as T;
        },
        tryGet: <T>(value: InjectionToken<T>): T | undefined => scope.tryGet(value),
      };
      return bindDeferredSlot(deferredScope, token);
    }
    if (multi) return scope.resolveAll(token);
    if (!optional) return scope.resolveEither(token);
    const absent = (error: unknown): undefined => {
      if (error instanceof NotRegisteredError && error.token === token && !this.host.isRegistered(scope, token)) return undefined;
      throw error;
    };
    try {
      const out = scope.resolveEither(token);
      return out instanceof Promise ? out.catch(absent) : out;
    } catch (error) { return absent(error); }
  }
  /**
   * Applies the members, method wraps and instance hooks a class declares to `instance`, once per instance. Synchronous
   * unless a member's service or a hook is asynchronous.
   */
  private complete<T>(instance: T, ctor: Constructor<T>, descriptor: ClassDescriptor, scope: Scope): T | Promise<T> {
    if (!isObject(instance)) return instance;
    const states = this.host.states(scope);
    let state = states.get(instance);
    if (state?.done) return instance;
    const lineage = currentCompletion(scope.completion);
    if (state) for (let link = lineage; link; link = currentCompletion(link.parent)) {
      const attempt = link.attempt;
      if (attempt.root === states && attempt.state === state && attempt.instance === instance && state.members === descriptor.members.length && state.wrapped) return instance;
    }
    // The weak completion entry is keyed by this exact supplied instance, so retry/join preserves T.
    if (state?.pending) return state.pending as Promise<T>;
    if (state?.active) return instance;
    if (!state) { state = { members: 0, wrapped: false, hooks: new WeakSet(), active: false, done: false }; states.set(instance, state); }
    const progress = state;
    progress.active = true;
    const attempt: CompletionAttempt = { root: states, state: progress, instance, active: true };
    progress.attempt = attempt;
    const end = (): void => { attempt.active = false; attempt.instance = undefined; progress.attempt = undefined; progress.active = false; progress.pending = undefined; };
    const finish = (): T => { progress.done = true; end(); return instance; };
    const fail = (error: unknown): never => { end(); throw error; };
    try {
      const members = descriptor.members;
      const start = progress.members;
      const out = then(gather(members.length - start, i => this.slot(scope, members[start + i]!.token, members[start + i]!.optional, members[start + i]!.multi)), fields => {
        for (let i = 0; i < fields.length; i++) {
          const member = members[start + i]!;
          member.set(instance, member.derive ? member.derive(fields[i], instance) : fields[i]);
          progress.members++;
        }
        if (!progress.wrapped) {
          const origins = methodOrigins.get(instance) ?? new Map<string | symbol, MethodOrigin>();
          methodOrigins.set(instance, origins);
          const wrapped = new Map<string | symbol, (...args: unknown[]) => unknown>();
          for (const wrap of descriptor.wraps) {
            let method = wrapped.get(wrap.name);
            if (!method) {
              // Wrap metadata names a method; callable checks below guard stale or overridden members.
              const current = (instance as Record<string | symbol, (...args: unknown[]) => unknown>)[wrap.name];
              if (typeof current !== 'function') throw new TypeError(`Cannot wrap ${ctor.name}.${String(wrap.name)}: not a method`);
              let origin = origins.get(wrap.name);
              if (!origin || !origin.generated.has(current)) { origin = { original: current, generated: new WeakSet() }; origins.set(wrap.name, origin); }
              method = origin.original.bind(instance);
            }
            wrapped.set(wrap.name, wrap.wrap(method, { instance, name: wrap.name }));
          }
          for (const [name, method] of wrapped) {
            Object.defineProperty(instance, name, { value: method, writable: true, configurable: true });
            origins.get(name)!.generated.add(method);
          }
          progress.wrapped = true;
        }
        const hooks = this.host.hooks(scope);
        return each(hooks, item => {
          if (progress.hooks.has(item)) return;
          const methods = descriptor.methods.filter(m => m.decorator === item.key);
          if (!methods.length && !descriptor.tags.includes(item.key)) return;
          const link: CompletionLink = { attempt, parent: lineage, valid: true, context: () => scope.resolutionContext?.() ?? this.host.context(scope) };
          /** Contextual operations are checked here; shared target forwarding supplies the remaining members. */
          const view = this.host.hookView(scope, link);
          const expire = (): void => { link.valid = false; link.context = undefined; };
          try {
            const out = item.hook(instance, { methods, descriptor, container: view });
            if (out instanceof Promise) return out.then(() => { expire(); progress.hooks.add(item); }, error => { expire(); throw error; });
            expire(); progress.hooks.add(item);
          } catch (error) { expire(); throw error; }
        });
      });
      if (out instanceof Promise) {
        const pending = out.then(() => shield(finish()), fail);
        progress.pending = pending;
        return pending;
      }
      return finish();
    } catch (error) { return fail(error); }
  }
  /** Builds a class that is not registered: `args` fill its first constructor parameters, the container resolves the rest. */
  build<T>(ctor: Constructor<T>, args: readonly unknown[], scope: Scope): T | Promise<T> {
    this.host.active();
    const descriptor = describe(ctor);
    return this.construct(scope, ctor, descriptor, descriptor.params, args);
  }
  /** Completes an instance the container did not construct. */
  fill<T extends object>(instance: T, scope: Scope): T | Promise<T> {
    this.host.active();
    // The constructor describes this supplied instance; it is inspected here, not invoked.
    const ctor = instance.constructor as Constructor<T>;
    return this.complete(instance, ctor, describe(ctor), scope);
  }
}
