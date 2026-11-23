import os,re
for name in ['container','host']:
 p=f'src/core/cminject.{name}.ts';dest='../../../.archive/2026-10-27/revisions/g05-step4/packages/foundation/inject/'+p;os.makedirs(os.path.dirname(dest),exist_ok=True)
 if not os.path.exists(dest):open(dest,'w').write(open(p).read())
p='src/core/cminject.container.ts';s=open(p).read();type_chunks=[]
for start,end in [('/** A build in flight.', '/** The context of a resolution that starts outside'),('/** Progress of one instance', '/** Skips expired hooks'),('/** What building a class needs', '/** The container that holds a scope'),('/** An instance hook subscribed', '/** Declared dependency metadata'),('/** Private construction and resolution operations', '/** Hook-local context')]:
 a=s.index(start);b=s.index(end,a);type_chunks.append(s[a:b]);s=s[:a]+s[b:]
a=s.index('/** Explicit, compiler-checked public forwarding');views=s[a:];s=s[:a]
views=views.replace('/** Preserve the historical one-way instanceof relationship without transplanting methods. */\nObject.setPrototypeOf(ContainerView.prototype, Container.prototype);','')
for name in ['ContainerView','ResolutionView','CompletionView']:
 views=views.replace('class '+name, 'export class '+name).replace('abstract export class '+name,'export abstract class '+name)
# Export abstract class modifier order is different from ordinary classes.
views=views.replace('abstract class ContainerView', 'export abstract class ContainerView').replace('export export abstract','export abstract')
views=views.replace('target: Container', 'target: ViewTarget').replace('target: Container;', 'target: ViewTarget;')
views=views.replace('const ctor = token instanceof DelayedConstructor ? token.get() : token;\n    if (!isConstructor(ctor)) throw new TypeError(`${tokenName(token)} needs a factory`);', 'const ctor = this.host.implementation(token);')
views=views.replace('return syncValue(this.host.injectMembers(instance, this), `Injecting ${instance.constructor.name}`, \'injectMembersAsync\');', 'return this.host.injectMembers(instance, this);').replace('return settle(() => this.host.injectMembers(instance, this));','return this.host.injectMembersAsync(instance, this);').replace('return syncValue(this.host.instantiate(ctor, args, this), `Building ${ctor.name}`, \'instantiateAsync\');','return this.host.instantiate(ctor, args, this);').replace('return settle(() => this.host.instantiate(ctor, args, this));','return this.host.instantiateAsync(ctor, args, this);')
views=views.replace('rootResolver(undefined, this.completion)', 'this.host.freshContext(this.completion)').replace('currentCompletion(this.link)', 'this.host.currentCompletion(this.link)')
# Reuse exact public type imports, all erased in the view module.
a=s.index('import {\n  ScopeType');b=s.index("from '@coldmind/inject/src/core/cminject.contracts.ts';",a)+len("from '@coldmind/inject/src/core/cminject.contracts.ts';")
contracts=s[a:b].replace('import {','import type {',1)
prefix=contracts+"\nimport type { Constructor, InjectionToken } from '@coldmind/inject/src/core/cminject.token.ts';\nimport type { Binding, StoredRecord, Build, Frame, Resolver, Scope, CompletionLink, ContextHost, ViewTarget } from '@coldmind/inject/src/core/cminject.host.ts';\n\n"
open('src/core/cminject.view.ts','w').write(prefix+views)
hp='src/core/cminject.host.ts';old=open(hp).read();old=old[old.index('/** Capabilities retained'):]
chunks='\n'.join(type_chunks)
chunks=chunks.replace('target: Container;', 'target: IContainer;').replace('root: Container;', 'root: WeakMap<object, CompletionState>;')
for name in ['Build','Frame','Resolver','CompletionState','CompletionAttempt','CompletionLink','Scope','HookSlot','ContextHost']:
 chunks=chunks.replace('interface '+name+' ', 'export interface '+name+' ')
chunks=chunks.replace('export interface ContextHost {', '''export interface ContextHost {
  /** Normalizes explicit lineage without importing the construction runtime into views. */
  currentCompletion(link?: CompletionLink): CompletionLink | undefined;
  /** Root context for a retained view whose original binding frame has expired. */
  freshContext(completion?: CompletionLink): Resolver;
  /** Validates the implementation side of a constructor registration overload. */
  implementation<T>(token: InjectionToken<T>): Constructor<T>;''')
chunks=chunks.replace('injectMembers<T extends object>(instance: T, scope: Scope): T | Promise<T>;', '''injectMembers<T extends object>(instance: T, scope: Scope): T;
  /** Asynchronous contextual member completion. */
  injectMembersAsync<T extends object>(instance: T, scope: Scope): Promise<T>;''').replace('instantiate<T>(ctor: Constructor<T>, args: readonly unknown[], scope: Scope): T | Promise<T>;', '''instantiate<T>(ctor: Constructor<T>, args: readonly unknown[], scope: Scope): T;
  /** Asynchronous contextual construction. */
  instantiateAsync<T>(ctor: Constructor<T>, args: readonly unknown[], scope: Scope): Promise<T>;''')
ht=contracts+"\nimport type { Constructor, InjectionToken } from '@coldmind/inject/src/core/cminject.token.ts';\nimport type { DiError } from '@coldmind/inject/src/core/cminject.errors.ts';\nimport type { Binding, StoredRecord } from '@coldmind/inject/src/core/cminject.binding.ts';\nexport type { Binding, StoredRecord };\n\n"+old+'\n'+chunks+'''\n/** The actual public target, including historically forwarded Container runtime extras. */
export interface ViewTarget extends IContainer {
  /** Tracked teardown is a public Container method, though not part of ordinary IContainer. */
  disposeWithTracking(): Promise<DisposalInfo[]>;
  /** Existing runtime-start rollback remains available on factory views. */
  rollbackNewInstances(preexisting: ReadonlySet<string>): Promise<void>;
}
'''
open(hp,'w').write(ht)
s="import { ContainerView, ResolutionView, CompletionView } from '@coldmind/inject/src/core/cminject.view.ts';\n"+s
s=s.replace('type { BindingOwner, RecordOwner }', 'type { BindingOwner, RecordOwner, Build, Frame, Resolver, CompletionState, CompletionAttempt, CompletionLink, Scope, HookSlot, ContextHost }')
s=s.replace('''  if (scope instanceof ContainerView) return scope.target;
  if (scope instanceof Container) return scope;''', '''  const target = scope instanceof ContainerView ? scope.target : scope;
  if (target instanceof Container) return target;''')
s=s.replace('attempt.root === root', 'attempt.root === states').replace('const attempt: CompletionAttempt = { root,', 'const attempt: CompletionAttempt = { root: states,')
s=s.replace('return this.contextHostState ??= {\n      context:', '''return this.contextHostState ??= {
      currentCompletion,
      freshContext: completion => rootResolver(undefined, completion),
      implementation<T>(token: InjectionToken<T>): Constructor<T> {
        const ctor = token instanceof DelayedConstructor ? token.get() : token;
        if (!isConstructor(ctor)) throw new TypeError(`${tokenName(token)} needs a factory`);
        return ctor;
      },
      context:''')
s=s.replace('injectMembers: (instance, scope) => target.fill(instance, scope),', '''injectMembers: (instance, scope) => syncValue(target.fill(instance, scope), `Injecting ${instance.constructor.name}`, 'injectMembersAsync'),
      injectMembersAsync: (instance, scope) => settle(() => target.fill(instance, scope)),''').replace('instantiate: (ctor, args, scope) => target.buildClass(ctor, args, scope),', '''instantiate: (ctor, args, scope) => syncValue(target.buildClass(ctor, args, scope), `Building ${ctor.name}`, 'instantiateAsync'),
      instantiateAsync: (ctor, args, scope) => settle(() => target.buildClass(ctor, args, scope)),''')
s+='\n/** The sole one-way instanceof attachment; view modules never import Container at runtime. */\nObject.setPrototypeOf(ContainerView.prototype, Container.prototype);\n'
open(p,'w').write(s)
