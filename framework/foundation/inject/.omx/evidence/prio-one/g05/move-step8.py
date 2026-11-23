import os,re,json
for name in ['container','host']:
 p=f'src/core/cminject.{name}.ts';dest='../../../.archive/2026-10-27/revisions/g05-step8/packages/foundation/inject/'+p;os.makedirs(os.path.dirname(dest),exist_ok=True)
 if not os.path.exists(dest):open(dest,'w').write(open(p).read())
p='src/core/cminject.container.ts';s=open(p).read();members=json.load(open('.omx/evidence/prio-one/g05/members.json'));globals=json.load(open('.omx/evidence/prio-one/g05/globals.json'));nodes=json.load(open('.omx/evidence/prio-one/g05/nodes.json'))
names={'addGraphNode','rebuildGraphKey','observe','observeResource','forgetObserved','discover','getScopeChain','toDescriptor','bindingOf','multiBindingsOf','instanceState','cachedBindings','recordPublic','ownershipRecords','dependencyEdges','getDependencyGraphSnapshot','getDependencyGraph','getDependencyTree','getResourceGraphSnapshot','graph','recordIssue','clearGraphIssues','restoreGraphIssues','issuesObserved','validate'}
selected=[m for m in members if m['name'] in names];assert len(selected)==len(names)
states={'observed','resourceObserved','observedPairs','resourcePairs','graphIssues','graphIssueKeys','graphState','graphQueue','graphIndexed','diGraph'};fields=[m for m in nodes if m['name'] in states];assert len(fields)==len(states)
gselected=[m for m in globals if m['name'] in {'exposeIssues','record'}];assert len(gselected)==2
methods=''.join(m['text'] for m in selected);data=''.join(m['text'] for m in fields);helpers=''.join(m['text'] for m in gselected)
for m in sorted(selected+fields+gselected,key=lambda m:m['start'],reverse=True):s=s[:m['start']]+s[m['end']:]
a=s.index('/** Diagnostic logs keep');b=s.index('/** Whether a value can carry',a);limit=s[a:b];s=s[:a]+s[b:]
# Observation owns only its diagnostic state; registry/records are read through explicit read facets.
for name in ['observe','observeResource','forgetObserved','rebuildGraphKey','restoreGraphIssues']:
 methods=methods.replace('private '+name+'(',name+'(')
for name in ['observed','resourceObserved','graphIssues','graphIssueKeys']:
 data=data.replace('private readonly '+name, 'readonly '+name)
data=data.replace('this.parent?.diGraph','this.host.parentGraph()').replace('order = this.order','order = this.host.order')
for old,new in [('this.assertActive()','this.host.active()'),('this.view()','this.host.view()'),('this.ancestors()','this.host.ancestors()'),('this.allDescendants()','this.host.descendants()'),('this.locateSingle(','this.host.selectSingle('),('this.locateMulti(','this.host.selectMulti('),('this.ownerOf(','this.host.ownerOf('),('this.bindingOwnerState','this.host.owner'),('this.isRegisteredRecursive(','this.host.isRegisteredRecursive('),('this.records','this.host.records'),('this.identity','this.host.identity'),('this.registry','this.host.registry'),('this.order','this.host.order')]:methods=methods.replace(old,new)
methods=methods.replace('for (let c: Container | undefined = this; c; c = c.parent)', 'for (let c: ObserveScope | undefined = this.host; c; c = c.parent)').replace('.locateSingle(dep.key)', '.selectSingle(dep.key)')
methods=methods.replace('validator.check({ ctor: binding.ctor, descriptor: binding.descriptor, container: this })', 'this.host.validator(binding, validator)')
helpers=helpers.replace('function exposeIssues','export function exposeIssues')
text="""import { ServiceLifetime, type BindingDescriptor, type CacheableBinding, type DiIssue, type DiscoveredClass, type InstanceRecord, type InstanceState, type MultiBindingEntry, type OwnershipRecord, type RuntimeEdge, type RuntimeDependencyEdge, type RuntimeDependencyGraph, type RuntimeDependencyNode, type RuntimeDependencyTreeNode, type RuntimeResourceEdge, type RuntimeResourceGraph, type RuntimeResourceNode, type ScopeTreeNode } from '@coldmind/inject/src/core/cminject.contracts.ts';
import { lookupToken, pinTokenKey, releaseTokenKey, retainTokenKey, tokenName, type InjectionToken } from '@coldmind/inject/src/core/cminject.token.ts';
import { DiGraph } from '@coldmind/inject/src/core/cminject.graph.ts';
import { DiGraphError } from '@coldmind/inject/src/core/cminject.errors.ts';
import { idOf, lifetimeText, type Binding, type StoredRecord } from '@coldmind/inject/src/core/cminject.binding.ts';
import type { ObserveHost, ObserveScope, Observed, ResourceObserved } from '@coldmind/inject/src/core/cminject.host.ts';
import { now, stamp } from '@coldmind/inject/src/core/cminject.async.ts';

"""+limit+helpers+'''\n/** Diagnostic indexing, bounded history and name/resource DTO projections; no registry mutation authority. */
export class Observation {
  /** Stable read/validator capabilities bound by the owning container. */
  constructor(private readonly host: ObserveHost) {}
'''+data+'''
  /** Internal indexing does not expose/pin every node key. */
  indexedGraph(): DiGraph { return this.diGraph; }
  /** Registration removal has already indexed its pre-mutation order. */
  finishIndex(): void { this.graphIndexed = this.host.order.length; }
  /** Releases graph/history state without rebuilding an unused graph during owner teardown. */
  disposeGraph(): void {
    this.graphQueue = undefined; this.graphIndexed = 0;
    if (this.graphState) for (const key of this.host.registry.keys()) this.graphState.remove(key, true);
    this.clearGraphIssues();
  }
  /** Instance clearing drops runtime observations but not historical declared-graph diagnostics. */
  clearRuntime(): void { this.observed.length = 0; this.resourceObserved.length = 0; this.forgetObserved(); }
'''+methods+'\n}\n'
open('src/core/cminject.observe.ts','w').write(text)
hp='src/core/cminject.host.ts';ht=open(hp).read().replace('  ScopeType, ServiceLifetime,', '  ScopeType, ServiceLifetime, ValidatorRecord,');ht="import type { DiGraph } from '@coldmind/inject/src/core/cminject.graph.ts';\n"+ht+'''\n/** Read-only scope projection for runtime/declared graph diagnostics. */
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
''';open(hp,'w').write(ht)
s="import { Observation, exposeIssues } from '@coldmind/inject/src/core/cminject.observe.ts';\n"+s
s=s.replace('ResourceObserved, MutationHost }', 'ResourceObserved, MutationHost, ObserveHost }')
# Existing stable mutation/resolution facets point at the same diagnostic owner after the move.
s=s.replace('target.observe(from, to)','target.observation.observe(from, to)').replace('target.observeResource(from, to)','target.observation.observeResource(from, to)').replace('target.observed','target.observation.observed').replace('target.resourceObserved','target.observation.resourceObserved').replace('target.graphIssues','target.observation.graphIssues').replace('target.graphIssueKeys','target.observation.graphIssueKeys').replace('void target.diGraph;', 'void target.observation.indexedGraph();').replace('target.rebuildGraphKey(key)','target.observation.rebuildGraphKey(key)').replace('target.graphIndexed = target.order.length;', 'target.observation.finishIndex();').replace('target.forgetObserved()','target.observation.forgetObserved()').replace('target.restoreGraphIssues(issues, keys)','target.observation.restoreGraphIssues(issues, keys)')
s=s.replace('''      this.graphQueue = undefined; this.graphIndexed = 0;
      if (this.graphState) for (const key of [...this.registry.keys()]) this.graphState.remove(key, true);
      this.clearGraphIssues();''','''      this.observationState?.disposeGraph();''').replace('this.observed.length = 0; this.resourceObserved.length = 0; this.forgetObserved();','this.observationState?.clearRuntime();')
# Public graph projections retain their original signatures; only their cohesive implementation moves.
public=['discover','getScopeChain','bindingOf','multiBindingsOf','instanceState','cachedBindings','ownershipRecords','dependencyEdges','getDependencyGraphSnapshot','getDependencyGraph','getDependencyTree','getResourceGraphSnapshot','graph','issuesObserved','validate']
wrappers=''
for name in public:
 m=next(m for m in selected if m['name']==name);body=m['text'];start=body.index('  '+name);signature=body[start:body.index('{',start)].rstrip()
 # Object-shaped return annotations contain braces; preserve the one special signature directly.
 if name=='getDependencyGraph':signature='  getDependencyGraph(): { from: string; to: string }[]'
 args='key' if name=='discover' else 'token' if name in {'bindingOf','multiBindingsOf','instanceState','getDependencyTree'} else ''
 wrappers+='  /** '+{'discover':'Root-first class discovery through read-only metadata.','validate':'Checks declarations and existing validators, preserving actual issue-key exposure.'}.get(name,'Runtime/declared diagnostic projection, without exposing private ownership state.')+' */\n'+signature+' { return this.observation.'+name+'('+args+'); }\n'
pos=s.index('  /** Initializes registrations actually owned')
s=s[:pos]+wrappers+'\n'+s[pos:]
# A single cached read facet prevents per-query capability-table allocation.
pos=s.index('  /** Stable mutation authority')
s=s[:pos]+'''  /** Read-only observer capabilities have stable scope identity across cross-scope projections. */
  private observeHostState?: ObserveHost;
  private get observeHost(): ObserveHost {
    if (!this.observeHostState) {
      const target = this;
      this.observeHostState = {
        get parent() { return target.parent?.observeHost; }, get identity() { return target.identity; },
        get epoch() { return target.epoch; }, get owner() { return target.bindingOwnerState; },
        get registry() { return target.registry; }, get order() { return target.order; }, get records() { return target.records; },
        get observed() { return target.observation.observed; }, get resourceObserved() { return target.observation.resourceObserved; },
        active: () => target.assertActive(),
        ancestors: () => target.ancestors().map(scope => scope.observeHost), view: () => target.view().map(scope => scope.observeHost),
        descendants: () => target.allDescendants().map(scope => scope.observeHost), ownerOf: binding => target.ownerOf(binding).observeHost,
        selectSingle: key => target.locateSingle(key), selectMulti: key => target.locateMulti(key),
        isRegisteredRecursive: token => target.isRegisteredRecursive(token), parentGraph: () => target.parent?.observation.indexedGraph(),
        validator(binding, validator) {
          if (!binding.ctor || !binding.descriptor) return;
          return validator.check({ ctor: binding.ctor, descriptor: binding.descriptor, container: target });
        },
      };
    }
    return this.observeHostState;
  }
  /** Graph state and weak observation indexes exist only for scopes that use them. */
  private observationState?: Observation;
  private get observation(): Observation { return this.observationState ??= new Observation(this.observeHost); }

'''+s[pos:]
open(p,'w').write(s)
