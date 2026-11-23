from pathlib import Path
import re,json
methods=['singleton','transient','scoped','tenant','request','singletonMulti','transientMulti','scopedMulti','tenantMulti','requestMulti']
f=Path('src/core/cminject.view.ts');s=f.read_text();s="import type { DependencySlot } from '@coldmind/inject/lazy';\nimport type { InjectedFactory, InjectedLifecycle, ExactFactoryOptions, RegistrySnapshot, ValidationOptions, InjectedSpec } from '@coldmind/inject/contracts';\nimport type { ModuleDefinition, ModuleHandle } from '@coldmind/inject/module';\n"+s
for name in methods:
 pattern=re.compile(r'(^  '+name+r'<T, D = void>[^\n]+)',re.M);m=pattern.search(s)
 if m:s=s[:m.start()]+f'  {name}<T, const Slots extends readonly DependencySlot[], Factory extends InjectedFactory<T, Slots>>(token: InjectionToken<T>, lifecycle: InjectedLifecycle<T, Slots, Factory>, ...options: ExactFactoryOptions<Slots, Factory>): BindingHandle<T>;\n'+s[m.start():];s=pattern.sub(lambda m:m[0].replace('ServiceLifecycle<T, D>',"(ServiceLifecycle<T, D> & { readonly inject?: never })"),s)
s=s.replace('  validate(): void { this.target.validate(); }','  validate(options?: ValidationOptions): void { this.target.validate(options); }');pos=s.index('\n',s.index('export abstract class ContainerView'))+1
s=s[:pos]+'''  load(...modules: ModuleDefinition[]): Promise<ModuleHandle[]> { return this.target.load(...modules); }
  snapshot(): RegistrySnapshot { return this.target.snapshot(); }
  restore(snapshot: RegistrySnapshot): Promise<void> { return this.target.restore(snapshot); }
'''+s[pos:];f.write_text(s)
# Runtime input erasure keeps all public overloads checked above it.
f=Path('src/core/cminject.contracts.ts');s=f.read_text();s=s.replace('export type InjectedFactory', '''/** Operational shape after the public tuple overload has checked every argument. */
export type InjectedSpec<T = unknown> = { readonly inject: readonly DependencySlot[]; factory: (...args: never[]) => T | Promise<T> } & Pick<ServiceLifecycle<T>, 'onInit' | 'onDispose' | 'onHealthCheck'>;
export type InjectedFactory''',1);f.write_text(s)
for name in ['container','view']:
 f=Path('src/core/cminject.'+name+'.ts');s=f.read_text()
 if name=='container':s="import type { InjectedSpec } from '@coldmind/inject/contracts';\n"+s
 for method in methods:
  s=s.replace(method+'<T, D>(token: InjectionToken<T>, factory?: ServiceFactory<T, D> | ServiceLifecycle<T, D> | RegisterOptions',method+'<T, D>(token: InjectionToken<T>, factory?: ServiceFactory<T, D> | ServiceLifecycle<T, D> | InjectedSpec<T> | RegisterOptions')
 f.write_text(s)
# Deferred slot integration in all authored metadata and construction.
f=Path('src/core/cminject.decorators.ts');s=f.read_text();s="import { isDeferredToken, type DependencySlot } from '@coldmind/inject/lazy';\n"+s;s=s.replace('token?: InjectionToken; optional:', 'token?: DependencySlot; optional:');s=s.replace("if (value && typeof value === 'object'", "if (isDeferredToken(value)) return { token: value, optional: false, multi: false };\n  if (value && typeof value === 'object'",1)
# Inject/InjectAll decorator parameters and metadata utility inputs, not registration token definitions.
s=s.replace('token: InjectionToken,', 'token: DependencySlot,').replace('token: InjectionToken)', 'token: DependencySlot)');f.write_text(s)
f=Path('src/core/cminject.construction.ts');s=f.read_text();s="import { isDeferredToken, unwrapDependencySlot, bindDeferredSlot, type DependencySlot } from '@coldmind/inject/lazy';\n"+s
s=s.replace('token: p.token!, index:', 'token: unwrapDependencySlot(p.token!), lazy: isDeferredToken(p.token), index:').replace('token: m.token, index:', 'token: unwrapDependencySlot(m.token), lazy: isDeferredToken(m.token), index:')
s=s.replace('private slot(scope: Scope, token: InjectionToken | undefined', 'private slot(scope: Scope, token: DependencySlot | undefined')
s=s.replace('    if (!token) return undefined;\n    if (multi)', '''    if (!token) return undefined;
    if (isDeferredToken(token)) {
      const deferredScope = {
        resolve: <T>(value: InjectionToken<T>): Promise<T> => Promise.resolve(scope.resolveEither(value) as T),
        resolveSync: <T>(value: InjectionToken<T>): T => {
          const out = scope.resolveEither(value);
          if (out instanceof Promise) throw new DiError('Deferred getSync requires synchronous construction', 'DI_LIFECYCLE');
          return out as T;
        },
        tryGet: <T>(value: InjectionToken<T>): T | undefined => scope.tryGet(value),
      };
      return bindDeferredSlot(deferredScope, token);
    }
    if (multi)''')
pos=s.index('  /**\n   * Builds `ctor`');s=s[:pos]+'''  /** Resolves an authored factory tuple without forcing synchronous dependencies async. */
  dependencies(container: IContainer, slots: readonly DependencySlot[]): unknown[] | Promise<unknown[]> {
    const scope = this.host.scope(container);
    return gather(slots.length, index => this.slot(scope, slots[index], false, false));
  }
'''+s[pos:];f.write_text(s)
f=Path('src/core/cminject.host.ts');s=f.read_text();s="import type { DependencySlot } from '@coldmind/inject/lazy';\n"+s;s=s.replace('  readonly signal: AbortSignal;\n  /** Explicit completion lineage','  readonly signal: AbortSignal;\n  tryGet<T>(token: InjectionToken<T>): T | undefined;\n  /** Explicit completion lineage');s=s.replace('  makeRecord(binding: Binding, data: unknown): StoredRecord;','  factoryArgs(container: IContainer, slots: readonly DependencySlot[]): unknown[] | Promise<unknown[]>;\n  makeRecord(binding: Binding, data: unknown): StoredRecord;');f.write_text(s)
f=Path('src/core/cminject.container.ts');s=f.read_text();s=s.replace('resolveEither: token => target.resolveEither(token), resolveAll:', 'tryGet: token => target.tryGet(token), resolveEither: token => target.resolveEither(token), resolveAll:');s=s.replace('        classFactory:', '        factoryArgs: (container, slots) => target.construction.dependencies(container, slots),\n        classFactory:');f.write_text(s)
f=Path('src/core/cminject.binding.ts');s=f.read_text();s="import { unwrapDependencySlot, isDeferredToken } from '@coldmind/inject/lazy';\n"+s;s=s.replace('  /** Per-registration deadline override. */\n  timeoutMs?', '  module?: string;\n  when?: RegisterOptions[\'when\'];\n  /** Per-registration deadline override. */\n  timeoutMs?',1);s=s.replace('  timeoutMs?: number | null;\n}\n\n\n/** Runtime','  timeoutMs?: number | null;\n}\n\n\n/** Runtime')
s=s.replace('export interface BindingOptions {','export interface BindingOptions {\n  module?: string;\n  when?: RegisterOptions[\'when\'];')
s=s.replace('  /** Per-registration deadline, including an explicit null disable value. */',"  get module(): string | undefined { return this.extra?.module; }\n  get when(): RegisterOptions['when'] { return this.extra?.when; }\n  /** Per-registration deadline, including an explicit null disable value. */")
s=s.replace("return this.kind === 'single' ? this.key : ((this.extra ??= {}).graphId ??= Symbol(`multi:${tokenName(this.token)}`));", "return this.kind === 'single' && !this.when ? this.key : ((this.extra ??= {}).graphId ??= Symbol(`candidate:${tokenName(this.token)}`));")
s=s.replace('({ token: dep, index, optional: false, multi: false })','({ token: unwrapDependencySlot(dep), lazy: isDeferredToken(dep), index, optional: false, multi: false })')
s=s.replace('const extra: BindingExtra | undefined = lifecycle || deps || declared || internal ||', 'const extra: BindingExtra | undefined = options?.when || lifecycle || deps || declared || internal ||');s=s.replace('? { kindTag, lifecycle, deps, declared, timeoutMs, owner:', '? { kindTag, lifecycle, deps, declared, timeoutMs, when: options?.when ?? internal?.when, module: internal?.module, owner:')
f.write_text(s)
f=Path('src/core/cminject.mutation.ts');s=f.read_text();s="import type { InjectedSpec } from '@coldmind/inject/contracts';\nimport { unwrapDependencySlot, isDeferredToken } from '@coldmind/inject/lazy';\nimport { chain } from '@coldmind/inject/internal/async';\n"+s
s=s.replace('factory: ServiceFactory<T, D> | ServiceLifecycle<T, D> | RegisterOptions | undefined,','factory: ServiceFactory<T, D> | ServiceLifecycle<T, D> | InjectedSpec<T> | RegisterOptions | undefined,',1)
needle='    // Common case: a plain factory';pos=s.index(needle);s=s[:pos]+'''    if (factory && typeof factory === 'object' && 'inject' in factory) {
      const definition = factory;
      const adapted: ServiceLifecycle<T, D> = {
        factory: (container: IContainer) => chain(this.host.factoryArgs(container, definition.inject), args => Reflect.apply(definition.factory, undefined, args) as T),
        onInit: definition.onInit, onDispose: definition.onDispose, onHealthCheck: definition.onHealthCheck,
      };
      return this.register(token, adapted, lifetime, kind, { ...options, deps: definition.inject }, internal);
    }
'''+s[pos:];s=s.replace('type BindingHandle,','type IContainer, type BindingHandle,',1)
s=s.replace('({ token, index, optional: false, multi: false })) : [...old.deps]', '({ token: unwrapDependencySlot(token), lazy: isDeferredToken(token), index, optional: false, multi: false })) : [...old.deps]')
s=s.replace('owner: old.owner, timeoutMs: old.timeoutMs', 'owner: old.owner, timeoutMs: old.timeoutMs, module: old.module, when: old.when')
f.write_text(s)
# Lazy edges are excluded only from eager traversal, never from captive checks.
f=Path('src/core/cminject.graph.ts');s=f.read_text();s=s.replace('for (const edge of node.deps) for (const target', 'for (const edge of node.deps.filter(edge => !edge.lazy)) for (const target');s=s.replace('    for (const edge of vertex.node.deps) {','    for (const edge of vertex.node.deps) {\n      if (edge.lazy) continue;');f.write_text(s)
Path('typedoc.json').write_text(json.dumps({'entryPoints':['src/core/cminject.core.ts','src/server/cminject.server.ts','src/cminject.testing.ts','src/cminject.inspect.ts','src/cminject.react.ts'],'out':'docs/api','excludePrivate':True,'excludeInternal':True},indent=2)+'\n')
print('Integrated deferred construction, typed factories and graph edge semantics; no checks run.')
