import os,re,json
for name in ['container','binding','host']:
 p=f'src/core/cminject.{name}.ts';dest='../../../.archive/2026-10-27/revisions/g05-step7/packages/foundation/inject/'+p;os.makedirs(os.path.dirname(dest),exist_ok=True)
 if not os.path.exists(dest):open(dest,'w').write(open(p).read())
p='src/core/cminject.container.ts';s=open(p).read();members=json.load(open('.omx/evidence/prio-one/g05/members.json'));globals=json.load(open('.omx/evidence/prio-one/g05/globals.json'))
names={'createBinding','install','register','registerConstructor','autoBinding','dependentBindings','awaitBuilds','recordsFor','discardRecord','replaceBinding','replace','rollbackNewInstances','unregister','unregisterBinding'}
selected=[m for m in members if m['name'] in names];assert len(selected)==len(names)
methods=''.join(m['text'] for m in selected if m['name']!='createBinding');definition=next(m['text'] for m in selected if m['name']=='createBinding')
gnames={'isConstructor','isLifecycle','checkTimeout','RegistrationSet','Observed','ResourceObserved'};gselected=[m for m in globals if m['name'] in gnames];assert len(gselected)==len(gnames)
for m in sorted(selected+gselected,key=lambda m:m['start'],reverse=True):s=s[:m['start']]+s[m['end']:]
# Pure definition preflight/retention belongs to the binding owner; no container capability is needed.
definition=definition.replace('  private createBinding<T, D>(token:', 'export function createBinding<T, D>(owner: BindingOwner, token:').replace('this.bindingOwner','owner')
definition=re.sub(r'internal\?: \{[^\n]+\}', 'internal?: BindingOptions',definition,count=1)
bp='src/core/cminject.binding.ts';bs=open(bp).read().replace('type ResourceLifetime,','type ResourceLifetime, type RegisterOptions,');bs+='''\n/** Metadata controlled by class registration and replacement staging. */
export interface BindingOptions {
  /** Unnormalized edges remain leased by the registration being constructed. */
  deps?: BindingDependency[];
  declared?: boolean;
  owner?: Binding['owner'];
  ctor?: Constructor;
  descriptor?: ClassDescriptor;
  generation?: number;
  timeoutMs?: number | null;
}
'''
for g in gselected:
 if g['name'] not in {'Observed','ResourceObserved'}:
  bs+='\n'+g['text'].replace('function '+g['name'], 'export function '+g['name']).replace('interface RegistrationSet','export interface RegistrationSet')
bs+='\n'+definition
# Definition creation's temporary lease is the only remaining intern operation in this module.
bs=bs.replace('type InjectionToken, lookupToken,','type InjectionToken, internToken, lookupToken,')
open(bp,'w').write(bs)
# Transform only authority accesses; mutation ordering/await boundaries stay byte-for-byte otherwise.
methods=methods.replace("internal?: Parameters<Container['createBinding']>[6]", 'internal?: BindingOptions')
methods=methods.replace('this.createBinding<T, D>(', 'createBinding<T, D>(this.host.owner, ').replace('this.createBinding(', 'createBinding(this.host.owner, ')
methods=methods.replace('this.construction.factory<T, D>(', 'this.host.classFactory<T, D>(').replace('this.construction.factory(', 'this.host.classFactory(')
for old,new in [('this.assertActive()','this.host.active()'),('this.registry','this.host.registry'),('this.order','this.host.order'),('this.epoch','this.host.epoch'),('this.bindingOwner','this.host.owner'),('this.autoInject','this.host.autoInject'),('this.lifetimeReason','this.host.lifetimeReason'),('this.subtree()','this.host.subtree()'),('this.view()','this.host.view()'),('this.nearestScope(','this.host.nearestScope('),('this.recordScope(','this.host.recordScope('),('this.locateMulti(','this.host.selectMulti('),('this.localSingle(','this.host.localSingle('),('this.parent','this.host.parent'),('this.handle','this.host.handle'),('this.disposeRecord(','this.host.disposeRecord('),('this.rebuildGraphKey(','this.host.rebuildGraphKey('),('this.resolution.entry(','this.host.entry(')]:methods=methods.replace(old,new)
methods=methods.replace('this.disposing || this.disposed','this.host.ended').replace('!this.disposing && !this.disposed','!this.host.ended').replace('snap.c.disposing || snap.c.disposed','snap.c.ended')
methods=methods.replace('scope: Container','scope: MutationScope').replace('scope: this,','scope: this.host,').replace('.resolution.entry(','.entry(').replace('void this.diGraph;', 'this.host.indexGraph();').replace('void c.diGraph;', 'c.indexGraph();').replace('this.graphIndexed = this.host.order.length;', 'this.host.finishGraphIndex();')
for name in ['install','register','autoBinding']:
 methods=methods.replace('private '+name+'(', name+'(').replace('private '+name+'<',name+'<')
text="""import { ScopeType, ServiceLifetime, type BindingHandle, type ClassDescriptor, type RegisterOptions, type ReplacementOptions, type ServiceFactory, type ServiceLifecycle, type ParameterInjection } from '@coldmind/inject/src/core/cminject.contracts.ts';
import { DelayedConstructor, type Constructor, type InjectionToken, internToken, lookupToken, tokenName, retainTokenKey, releaseTokenKey } from '@coldmind/inject/src/core/cminject.token.ts';
import { TypedBinding, createBinding, isConstructor, KIND_TAGS, type Binding, type BindingOptions, type StoredRecord, type AnyFactory } from '@coldmind/inject/src/core/cminject.binding.ts';
import type { MutationHost, MutationScope } from '@coldmind/inject/src/core/cminject.host.ts';
import { rootResolver } from '@coldmind/inject/src/core/cminject.resolution.ts';
import { classEdges } from '@coldmind/inject/src/core/cminject.construction.ts';
import { describe } from '@coldmind/inject/src/core/cminject.decorators.ts';
import { MutationConflictError, NotRegisteredError, ScopeMissingError } from '@coldmind/inject/src/core/cminject.errors.ts';
import { attachCause } from '@coldmind/inject/src/core/cminject.async.ts';

/** Definition installation and transactional replacement share one preflight/commit owner. */
export class Mutation {
  /** Stable exact-identity capabilities, not a mutable Container implementation. */
  constructor(private readonly host: MutationHost) {}
"""+methods+'\n}\n'
open('src/core/cminject.mutation.ts','w').write(text)
hp='src/core/cminject.host.ts';ht=open(hp).read();ht=ht.replace('import type { Binding, StoredRecord }', 'import type { Binding, StoredRecord, TypedBinding, RegistrationSet }')
for g in gselected:
 if g['name'] in {'Observed','ResourceObserved'}:ht+='\n'+g['text'].replace('interface '+g['name'],'export interface '+g['name'])
ht+='''\n/** Exact scope state needed while staging/rolling back one mutation transaction. */
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
  readonly owner: BindingOwner;
  readonly registry: Map<symbol, Binding | RegistrationSet>;
  readonly order: Binding[];
  readonly parent: MutationHost | undefined;
  readonly autoInject: boolean | AutoInjectOptions;
  readonly lifetimeReason: DiError | undefined;
  active(): void;
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
''';open(hp,'w').write(ht)
s="import { Mutation } from '@coldmind/inject/src/core/cminject.mutation.ts';\n"+s
s=s.replace('type BindingExtra, type BindingDependency, type AnyFactory', 'createBinding, checkTimeout, isConstructor, type RegistrationSet, type BindingExtra, type BindingDependency, type AnyFactory')
s=s.replace('HookSlot, ContextHost }', 'HookSlot, ContextHost, Observed, ResourceObserved, MutationHost }')
s=s.replace('return this.register(', 'return this.mutation.register(').replace('target.autoBinding(token, key)', 'target.mutation.autoBinding(token, key)').replace('this.install(binding)', 'this.mutation.install(binding)')
s=s.replace('this.createBinding<T, void>(', 'createBinding<T, void>(this.bindingOwner, ').replace('this.createBinding(', 'createBinding(this.bindingOwner, ')
# Per-scope capability identity is stable, so touched-scope deduplication and snapshots remain exact.
pos=s.index('  /** Slot-only manual scope')
s=s[:pos]+'''  /** Stable mutation authority is assembled once, with no private-field or nominal Container escape. */
  private mutationHostState?: MutationHost;
  private get mutationHost(): MutationHost {
    if (!this.mutationHostState) {
      const target = this;
      this.mutationHostState = {
        get scopeType() { return target.scopeType; }, get owner() { return target.bindingOwner; },
        get registry() { return target.registry; }, get order() { return target.order; },
        get records() { return target.records; }, get pending() { return target.pending; },
        get disposalOrder() { return target.disposalOrder; }, get observed() { return target.observed; },
        get resourceObserved() { return target.resourceObserved; }, get graphIssues() { return target.graphIssues; },
        get graphIssueKeys() { return target.graphIssueKeys; }, get ended() { return target.disposing || target.disposed; },
        get epoch() { return target.epoch; }, set epoch(value) { target.epoch = value; },
        get parent() { return target.parent?.mutationHost; }, get autoInject() { return target.autoInject; },
        get lifetimeReason() { return target.lifetimeReason; },
        active: () => target.assertActive(),
        subtree: () => target.subtree().map(scope => scope.mutationHost), view: () => target.view().map(scope => scope.mutationHost),
        nearestScope: kind => target.nearestScope(kind)?.mutationHost,
        recordScope: record => target.recordScope(record).mutationHost,
        selectMulti: key => target.locateMulti(key), localSingle: key => target.localSingle(key),
        isRegisteredRecursive: token => target.isRegisteredRecursive(token),
        replace: (token, factory, lifetime, options) => target.mutation.replace(token, factory, lifetime, options),
        classFactory: (ctor, descriptor, params) => target.construction.factory(ctor, descriptor, params),
        handle: (binding, retained) => target.handle(binding, retained),
        entry: (binding, data, context) => target.resolution.entry(binding, data, context),
        route: binding => target.route(binding).mutationHost,
        disposeRecord: (record, tracked) => target.disposeRecord(record, tracked),
        indexGraph: () => { void target.diGraph; }, rebuildGraphKey: key => target.rebuildGraphKey(key),
        finishGraphIndex: () => { target.graphIndexed = target.order.length; },
        forgetObserved: () => target.forgetObserved(), restoreGraphIssues: (issues, keys) => target.restoreGraphIssues(issues, keys),
      };
    }
    return this.mutationHostState;
  }
  /** Registration and replacement use the same transaction engine, allocated only when needed. */
  private mutationState?: Mutation;
  private get mutation(): Mutation { return this.mutationState ??= new Mutation(this.mutationHost); }

'''+s[pos:]
pos=s.rindex('\n}')
s=s[:pos]+'''
  /** Transactional replacement, with the existing resolution engine controlling candidates. */
  replaceBinding<T, D = void>(bindingId: string, factory: ServiceFactory<T, D> | ServiceLifecycle<T, D>, lifetime?: ServiceLifetime, options?: ReplacementOptions): Promise<BindingHandle<T, D>> { return this.mutation.replaceBinding(bindingId, factory, lifetime, options); }
  /** Replaces the selected single definition without bypassing normal ownership/rollback policy. */
  replace<T, D = void>(token: InjectionToken<T>, factory: ServiceFactory<T, D> | ServiceLifecycle<T, D>, lifetime: ServiceLifetime, options?: ReplacementOptions): Promise<BindingHandle<T, D>> { return this.mutation.replace(token, factory, lifetime, options); }
  /** @internal Runtime startup rollback shares the central mutation/disposal authority. */
  rollbackNewInstances(preexisting: ReadonlySet<string>): Promise<void> { return this.mutation.rollbackNewInstances(preexisting); }
  /** Removes only registrations owned by this scope. */
  unregister(token: InjectionToken): Promise<void> { return this.mutation.unregister(token); }
  /** Removes exactly one selected binding generation. */
  unregisterBinding(bindingId: string): Promise<boolean> { return this.mutation.unregisterBinding(bindingId); }
'''+s[pos:]
open(p,'w').write(s)
