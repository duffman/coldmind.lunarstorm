import os,re
for name in ['container','binding','host']:
 p=f'src/core/cminject.{name}.ts';dest='../../../.archive/2026-10-27/revisions/g05-step5/packages/foundation/inject/'+p;os.makedirs(os.path.dirname(dest),exist_ok=True)
 if not os.path.exists(dest):open(dest,'w').write(open(p).read())
p='src/core/cminject.container.ts';s=open(p).read()
a=s.index('/** Skips expired hooks');b=s.index('/** Original method',a);normalize=s[a:b];s=s[:a]+s[b:]
a=s.index('/** Original method');b=s.index('/** The container that holds a scope',a);origins=s[a:b];s=s[:a]+s[b:]
a=s.index('/** Declared dependency metadata');b=s.index('/** Whether a registration argument',a);edges=s[a:b];s=s[:a]+s[b:]
a=edges.index('/** Declared dependencies of a class');alias=edges[:a];edges=edges[a:]
bp='src/core/cminject.binding.ts';bs=open(bp).read();bs+='\n'+alias.replace('type BindingDependency','export type BindingDependency');open(bp,'w').write(bs)
a=s.index('  /** Factory that constructs');b=s.index('  /** Stable typed capabilities',a);methods=s[a:b];s=s[:a]+s[b:]
a=s.index('  /** Builds a class that is not registered');b=s.index('  /** Builds `ctor` without registering',a);build=s[a:b];s=s[:a]+s[b:]
s=s.replace('  /** Root-owned completion progress, weakly keyed so completing supplied objects never owns their lifetime. */\n  private completions?: WeakMap<object, CompletionState>;\n','')
# Remove field and its existing one-line documentation, whatever its exact wording.
s=re.sub(r'  /\*\*[^\n]*\*/\n  private completions\?: WeakMap<object, CompletionState>;\n','',s)
a=s.index('/** Constructor arguments of a build that supplies none.');b=s.index('/**',s.index('\n',s.index('const NO_ARGS',a))+1);noargs=s[a:b];s=s[:a]+s[b:]
methods=methods.replace('private classFactory<','factory<').replace('''      if (!(scope instanceof ContainerView) && !(scope instanceof Container)) throw new LifecycleError('Unknown class factory context');
      return this.construct(scope,''','''      return this.construct(this.host.scope(scope),''')
methods=methods.replace('!hostOf(scope).isRegisteredRecursive(token)', '!this.host.isRegistered(scope, token)').replace('''    const root = hostOf(scope).ancestors()[0]!;
    const states = root.completions ??= new WeakMap();''','''    const states = this.host.states(scope);''').replace('const hooks = hostOf(scope).ancestors().flatMap(holder => holder.instanceHooks);','const hooks = this.host.hooks(scope);').replace('          const target = hostOf(scope);\n','').replace('scope.resolutionContext?.() ?? target.rootCtx()', 'scope.resolutionContext?.() ?? this.host.context(scope)').replace('target.completionView(link, () => scope.signal)', 'this.host.hookView(scope, link)')
build=build.replace('private buildClass<','build<').replace('private fill<','fill<').replace('scope: Scope = this','scope: Scope').replace('this.assertActive();','this.host.active();')
normalize=normalize.replace('function currentCompletion','export function currentCompletion');edges=edges.replace('function classEdges','export function classEdges')
text="""import type { ClassDescriptor, ParameterInjection, ServiceFactory } from '@coldmind/inject/src/core/cminject.contracts.ts';
import type { Constructor, InjectionToken } from '@coldmind/inject/src/core/cminject.token.ts';
import type { BindingDependency } from '@coldmind/inject/src/core/cminject.binding.ts';
import type { CompletionState, CompletionAttempt, CompletionLink, Scope, ConstructionHost } from '@coldmind/inject/src/core/cminject.host.ts';
import { DiError, NotRegisteredError } from '@coldmind/inject/src/core/cminject.errors.ts';
import { describe } from '@coldmind/inject/src/core/cminject.decorators.ts';
import { chain as then, each, gather, shield } from '@coldmind/inject/src/core/cminject.async.ts';

"""+noargs+normalize+origins+edges+'''/** Supplied objects and functions can carry injected fields and methods. */
const isObject = (value: unknown): value is object => value !== null && (typeof value === 'object' || typeof value === 'function');

/** Constructor injection and root-owned completion; its host deliberately has no registry mutation capabilities. */
export class Construction {
  /** Contextual read/completion capabilities bound once by the owning scope. */
  constructor(private readonly host: ConstructionHost) {}
  /** Root progress exists only after the first completion, and never owns supplied instances strongly. */
  private completionStates?: WeakMap<object, CompletionState>;
  /** Root identity and progress shared by descendants through the narrow host. */
  get states(): WeakMap<object, CompletionState> { return this.completionStates ??= new WeakMap(); }
'''+methods+build+'}\n'
open('src/core/cminject.construction.ts','w').write(text)
hp='src/core/cminject.host.ts';ht=open(hp).read();ht+='''\n/** Constructor/slot/member completion authority; no registration or cache mutation operations. */
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
''';open(hp,'w').write(ht)
s="import { Construction, classEdges, currentCompletion } from '@coldmind/inject/src/core/cminject.construction.ts';\n"+s
s=s.replace('type BindingExtra, type AnyFactory', 'type BindingExtra, type BindingDependency, type AnyFactory')
s=s.replace("/** The container that holds a scope's instances. */\nfunction hostOf(scope: Scope): Container {\n  const target = scope instanceof ContainerView ? scope.target : scope;\n  if (target instanceof Container) return target;\n  throw new LifecycleError('Unknown internal resolution scope');\n}", """/** Manual construction uses a stable slot-only facade, never the mutable container instance. */
const scopeOwners = new WeakMap<Scope, Container>();
/** The core alone translates scope capabilities into its private hierarchy. */
function hostOf(scope: Scope): Container {
  if (scope instanceof ContainerView && scope.target instanceof Container) return scope.target;
  const target = scopeOwners.get(scope);
  if (target) return target;
  throw new LifecycleError('Unknown internal resolution scope');
}""")
pos=s.index('  /** Stable typed capabilities shared')
s=s[:pos]+'''  /** Slot-only manual scope, bound once and shared by unregistered construction/injection. */
  private constructionScopeState?: Scope;
  /** Manual construction does not hand the helper a mutable Container as a nominal Scope. */
  private get constructionScope(): Scope {
    if (!this.constructionScopeState) {
      const target = this;
      this.constructionScopeState = { get signal() { return target.signal; }, resolveEither: token => target.resolveEither(token), resolveAll: token => target.resolveAll(token) };
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

'''+s[pos:]
s=s.replace('this.classFactory<', 'this.construction.factory<').replace('this.classFactory(', 'this.construction.factory(').replace('target.fill(instance, scope)', 'target.construction.fill(instance, scope)').replace('target.buildClass(ctor, args, scope)', 'target.construction.build(ctor, args, scope)').replace('this.buildClass(ctor, args)', 'this.construction.build(ctor, args, this.constructionScope)').replace('this.fill(instance)', 'this.construction.fill(instance, this.constructionScope)')
open(p,'w').write(s)
