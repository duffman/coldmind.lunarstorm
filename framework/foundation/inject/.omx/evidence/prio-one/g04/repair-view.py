p='src/core/cminject.container.ts';s=open(p).read()
s=s.replace("function hostOf(scope: Scope): Container { return scope instanceof ResolutionView || scope instanceof CompletionView ? scope.target : scope as Container; }", """function hostOf(scope: Scope): Container {
  if (scope instanceof ContainerView) return scope.target;
  if (scope instanceof Container) return scope;
  throw new LifecycleError('Unknown internal resolution scope');
}""")
s=s.replace('private registerConstructor<T>(', 'private registerConstructor<T, D = void>(').replace("kind: 'single' | 'multi', tag?: string): BindingHandle<T> {", "kind: 'single' | 'multi', tag?: string): Binding<T, D> {")
s=s.replace("return this.registerConstructor(token, factory as RegisterOptions | undefined, lifetime, kind) as BindingHandle<T, D>;", "return this.registerConstructor<T, D>(token, factory as RegisterOptions | undefined, lifetime, kind);")
s=s.replace("const binding = this.createBinding<T, void>(token, this.classFactory(ctor, descriptor, params)", "const binding = this.createBinding<T, D>(token, this.classFactory(ctor, descriptor, params)")
s=s.replace("    return this.handle(binding);\n  }\n  /** Factory that constructs", "    return binding;\n  }\n  /** Factory that constructs")
s=s.replace('params: readonly ParameterInjection[]): ServiceFactory<T, void>', 'params: readonly ParameterInjection[]): (scope: IContainer) => T | Promise<T>')
s=s.replace("    return (c: IContainer) => this.construct(c as unknown as Scope, ctor, descriptor, params, NO_ARGS);", """    return (scope: IContainer) => {
      if (!(scope instanceof ContainerView) && !(scope instanceof Container)) throw new LifecycleError('Unknown class factory context');
      return this.construct(scope, ctor, descriptor, params, NO_ARGS);
    };""")
s=s.replace("const view: Pick<IContainer, 'resolve' | 'resolveSync' | 'resolveAll' | 'injectMembers' | 'injectMembersAsync' | 'instantiate' | 'instantiateAsync'> = target.completionView", "const view = target.completionView").replace('container: view as IContainer', 'container: view')
a=s.index('  /** Supplies a hook facade with typed callbacks')
b=s.index('  /** Builds a class that is not registered',a)
s=s[:a]+'''  /** Stable typed capabilities shared by both context facades, allocated once per container. */
  private contextHostState?: ContextHost;
  /** Private resolution/construction callbacks never expose the mutable container implementation. */
  private get contextHost(): ContextHost {
    const target = this;
    return this.contextHostState ??= {
      context: () => target.rootCtx(),
      resolve<T, D>(context: Resolver, token: InjectionToken<T>, data: D | undefined, known?: symbol): Promise<T> { return promised<T>(() => target.resolveIn(context, token, data, known)); },
      resolveSync<T, D>(context: Resolver, token: InjectionToken<T>, data: D | undefined, known?: symbol): T { return synchronous<T>(target.resolveIn(context, token, data, known), token); },
      resolveEither: (context, token, known) => then(target.resolveIn(context, token, undefined, known), record => record.instance),
      resolveAll: (context, token, data) => target.resolveAllIn(context, token, data),
      injectMembers: (instance, scope) => target.fill(instance, scope),
      instantiate: (ctor, args, scope) => target.buildClass(ctor, args, scope),
    };
  }
  /** Supplies a hook facade without a synthetic binding or per-hook operation table. */
  private completionView(link: CompletionLink, signal: () => AbortSignal): CompletionView {
    return new CompletionView(this, link, this.contextHost, signal);
  }
'''+s[b:]
s=s.replace("    const exposed: Pick<IContainer, 'resolve' | 'resolveSync' | 'resolveAll' | 'signal'> = view;\n    const container = exposed as IContainer;", "    const container: IContainer = view;")
s=s.replace('ctx.suppressInterceptors, ctx.completion);', 'ctx.suppressInterceptors, target.contextHost, ctx.completion);')
s=s.replace('interface CompletionHost {\n  /** The originating build signal remains stable after a hook link expires. */\n  signal(): AbortSignal;', 'interface ContextHost {')
s=s.replace('data: D | undefined): Promise<T>;', 'data: D | undefined, known?: symbol): Promise<T>;').replace('data: D | undefined): T;', 'data: D | undefined, known?: symbol): T;').replace('resolveEither(context: Resolver, token: InjectionToken): unknown;', 'resolveEither(context: Resolver, token: InjectionToken, known?: symbol): unknown;')
s=s.replace('  abstract resolveEither(token: InjectionToken): unknown;', '''  abstract resolveEither(token: InjectionToken): unknown;
  /** Typed contextual operations owned by the target. */
  protected abstract readonly host: ContextHost;
  /** Active resolver and completion lineage for contextual construction. */
  abstract resolutionContext(): Resolver;''')
# Move contextual construction wrappers to shared base so resolution views no longer silently use bare roots.
for name in ['injectMembers','injectMembersAsync','instantiate','instantiateAsync']:
 import re
 s=re.sub(r'  abstract '+name+r'[^\n]*\n', '', s)
a=s.index('  /** Applies members without losing the active completion lineage. */',s.index('class CompletionView'))
b=s.index('\n}',a)
wrappers=s[a:b]
s=s[:a]+s[b:]
pos=s.index('\n}\n\n/**\n * One build:')
s=s[:pos]+'\n'+wrappers+s[pos:]
s=s.replace('    staged: ReadonlyMap<symbol, Binding> | undefined, suppressInterceptors: boolean, completion?: CompletionLink) {', '    staged: ReadonlyMap<symbol, Binding> | undefined, suppressInterceptors: boolean, protected readonly host: ContextHost, completion?: CompletionLink) {')
s=s.replace("return promised<T>(() => this.target['resolveIn'](this.resolutionContext(), token, data, this.binding.keyOf(token)));", 'return this.host.resolve(this.resolutionContext(), token, data, this.binding.keyOf(token));').replace("return synchronous<T>(this.target['resolveIn'](this.resolutionContext(), token, data, this.binding.keyOf(token)), token);", 'return this.host.resolveSync(this.resolutionContext(), token, data, this.binding.keyOf(token));').replace("return then(this.target['resolveIn'](this.resolutionContext(), token, undefined, this.binding.keyOf(token)), record => record.instance);", 'return this.host.resolveEither(this.resolutionContext(), token, this.binding.keyOf(token));').replace("return this.target['resolveAllIn']<T>(this.resolutionContext(), token, data);", 'return this.host.resolveAll(this.resolutionContext(), token, data);')
s=s.replace('private readonly host: CompletionHost) { super(target); }', 'protected readonly host: ContextHost, private readonly signalSource: () => AbortSignal) { super(target); }').replace('return this.host.signal();', 'return this.signalSource();')
s=s.replace('private readonly binding: Binding, readonly retainedInstances: readonly T[]', 'private readonly binding: Binding<T, D>, readonly retainedInstances: readonly T[]').replace('return this.binding.token as InjectionToken<T>;', 'return this.binding.token;')
# Registration forwarding distinguishes factory form before narrowing the constructor-only form.
s=s.replace("typeof factory === 'function' || isLifecycle(factory)", "typeof factory === 'function' || (factory !== undefined && 'factory' in factory)")
open(p,'w').write(s)
