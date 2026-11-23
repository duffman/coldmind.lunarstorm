import os,re,json
for name in ['container','binding','host']:
 p=f'src/core/cminject.{name}.ts';dest='../../../.archive/2026-10-27/revisions/g05-step6/packages/foundation/inject/'+p;os.makedirs(os.path.dirname(dest),exist_ok=True)
 if not os.path.exists(dest):open(dest,'w').write(open(p).read())
p='src/core/cminject.container.ts';s=open(p).read();members=json.load(open('.omx/evidence/prio-one/g05/members.json'))
names={'makeRecord','runInit','materialize','constructAsync','initialize','ready','fail','discard','entry','finishBuild','cleanupLate','cancelledResult','stopBuild','timeoutError','watchBuild','linked','settled','share','resolve','rootCtx','resolveEither','resolveSync','resolveIn','resolveIntercepted','resolveAll','resolveAllIn'}
selected=[m for m in members if m['name'] in names];methods=''.join(m['text'] for m in selected)
assert len(selected)==len(names),(len(selected),len(names))
for m in reversed(selected):s=s[:m['start']]+s[m['end']:]
a=s.index('/** The context of a resolution that starts outside');b=s.index('/** Core-only mapping',a);helpers=s[a:b];s=s[:a]+s[b:]
a=s.index('/** Source of the resolution a running factory');b=s.index('/** Registrations of one token',a);ambient=s[a:b];s=s[:a]+s[b:]
a=s.index('/** How long a lifetime lives');b=s.index('/** Runtime construction check',a);span=s[a:b];s=s[:a]+s[b:]
a=s.index('/** Lifetime names used');b=s.index('/** Message of the',a);lifetime=s[a:b];s=s[:a]+s[b:]
bp='src/core/cminject.binding.ts';bs=open(bp).read().replace('type IContainer,','type IContainer, type ResourceLifetime,');bs+='\n'+lifetime.replace('const lifetimeText','export const lifetimeText')+span.replace('const lifespan','export const lifespan');open(bp,'w').write(bs)
helpers=helpers.replace('const ROOT:', 'export const ROOT:').replace('const rootResolver =','export const rootResolver =').replace('target: Container', 'target: { readonly identity: ScopeIdentity }')
# The helper owns every state transition, cache publication and terminal build control.
for name in ['makeRecord','entry','rootCtx','resolveIn','resolveAllIn']:
 methods=methods.replace('private '+name+'(', name+'(').replace('private async '+name+'<', 'async '+name+'<')
methods=methods.replace('makeRecord(binding: Binding, data: unknown, container: Container)', 'makeRecord(binding: Binding, data: unknown)').replace('new StoredRecord(binding, container.recordOwner, data, at)', 'new StoredRecord(binding, this.host.recordOwner, data, at)')
methods=methods.replace('target: Container', 'target: Resolution').replace('this.assertActive()', 'this.host.active()').replace('target.assertActive()', 'target.host.active()').replace('this.route(binding)', 'this.host.route(binding)').replace('target.makeRecord(binding, data, target)', 'target.makeRecord(binding, data)').replace('target.buildTimeout','target.host.buildTimeout')
methods=methods.replace('new ResolutionView(target, binding, record, parent, null, ctx.staged, ctx.suppressInterceptors, target.contextHost, ctx.completion)', 'target.host.view(binding, record, parent, ctx)')
methods=methods.replace('this.disposeInstance(', 'this.host.dispose(').replace('this.observeResource(', 'this.host.observeResource(').replace('this.observe(', 'this.host.observe(').replace('this.hasInterceptors()', 'this.host.hasInterceptors()').replace('this.gatherInterceptors()', 'this.host.interceptors()').replace('this.assertNotMulti(', 'this.host.assertSingle(').replace('this.locateSingle(', 'this.host.selectSingle(').replace('this.locateMulti(', 'this.host.selectMulti(').replace('this.autoBinding(', 'this.host.autoBinding(')
methods=methods.replace('''    const root = this.ancestors()[0]!;
    const pending = root.subtree().flatMap(scope => [...(scope.builds ?? [])]);''', '    const pending = this.host.allBuilds();')
methods=methods.replace('''    const buildingScope = binding && (binding.lifetime === ServiceLifetime.Singleton ? this.ownerOf(binding)
      : binding.lifetime === ServiceLifetime.Tenant ? this.nearestScope(ScopeType.Tenant)
      : binding.lifetime === ServiceLifetime.Request ? this.nearestScope(ScopeType.Request) : this);''', '    const scopeId = binding && this.host.scopeId(binding);').replace('buildingScope?.identity.id', 'scopeId').replace('this.identity.id','this.host.identity.id')
text="""import { ServiceLifetime, type IContainer, type ResolutionContext, type ResolutionPathStep, type ScopeIdentity } from '@coldmind/inject/src/core/cminject.contracts.ts';
import { lookupToken, normalizeToken, tokenName, type InjectionToken } from '@coldmind/inject/src/core/cminject.token.ts';
import { StoredRecord, idOf, lifetimeText, lifespan, type Binding } from '@coldmind/inject/src/core/cminject.binding.ts';
import type { Build, Frame, Resolver, CompletionLink, ResolutionHost } from '@coldmind/inject/src/core/cminject.host.ts';
import { ResolutionView } from '@coldmind/inject/src/core/cminject.view.ts';
import { currentCompletion } from '@coldmind/inject/src/core/cminject.construction.ts';
import { CircularDependencyError, DiError, DiGraphError, NotRegisteredError } from '@coldmind/inject/src/core/cminject.errors.ts';
import { now, stamp, chain as then, each, guarded, promised, synchronous, errorOf, attachCause } from '@coldmind/inject/src/core/cminject.async.ts';

"""+helpers+ambient+'''/** Materialized objects/functions may implement initialization methods. */
const isObject = (value: unknown): value is object => value !== null && (typeof value === 'object' || typeof value === 'function');

/** One owner's explicit resolution pipeline and single-owner terminal build control. */
export class Resolution {
  /** Stable selection/routing/observation capabilities, without arbitrary registration mutation. */
  constructor(private readonly host: ResolutionHost) {}
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
'''+methods+'\n}\n'
# Public graph-issue errors intentionally expose only the actual raw keys carried to callers.
text=text.replace('exposeIssues(', 'this.host.exposeIssues(')
open('src/core/cminject.resolution.ts','w').write(text)
hp='src/core/cminject.host.ts';ht=open(hp).read();ht="import type { Resolution } from '@coldmind/inject/src/core/cminject.resolution.ts';\nimport type { ResolutionView } from '@coldmind/inject/src/core/cminject.view.ts';\n"+ht+'''\n/** Selection/routing/control authority for one owner's resolution pipeline. */
export interface ResolutionHost {
  /** Scope metadata and inherited policy remain lazy. */
  readonly identity: ScopeIdentity;
  readonly buildTimeout: number | null | undefined;
  readonly recordOwner: RecordOwner;
  /** Active-state check precedes every new materialization. */
  active(): void;
  /** Routes only a selected registration to its lifetime owner. */
  route(binding: Binding): Resolution;
  /** Selects visible definitions, including transaction-local candidates. */
  selectSingle(key: symbol, staged?: ReadonlyMap<symbol, Binding>): Binding | undefined;
  selectMulti(key: symbol, staged?: ReadonlyMap<symbol, Binding>): Binding[];
  /** Existing class-only auto-registration policy, not arbitrary factory mutation. */
  autoBinding(token: InjectionToken, key: symbol): Binding | undefined;
  /** Preserves single/multi shadowing semantics before materialization. */
  assertSingle(key: symbol, binding: Binding, token: InjectionToken): void;
  /** Read-only interceptor enumeration. */
  hasInterceptors(): boolean;
  interceptors(): readonly Interceptor[];
  /** Scope metadata preview does not itself build a service. */
  scopeId(binding: Binding): string | undefined;
  /** View creation is the sole public-target boundary; helpers never receive private container state. */
  view(binding: Binding, record: StoredRecord, parent: Frame | undefined, context: Resolver): ResolutionView;
  /** Observes selected definitions and successful resource links. */
  observe(from: Binding, to: Binding): void;
  observeResource(from: StoredRecord, to: StoredRecord): void;
  /** Central lifecycle ownership/cleanup claim, shared with teardown. */
  dispose(record: StoredRecord, unreturned: boolean): unknown;
  /** Root diagnostic snapshot includes pending transients too. */
  allBuilds(): readonly ResolutionView[];
  /** Public errors pin only the issue keys actually exposed. */
  exposeIssues(issues: DiIssue[]): DiIssue[];
}
''';open(hp,'w').write(ht)
s="import { Resolution, rootResolver, useAmbientResolution, type AmbientResolution } from '@coldmind/inject/src/core/cminject.resolution.ts';\nexport { useAmbientResolution };\nexport type { AmbientResolution };\n"+s
s=s.replace('type BindingExtra, type BindingDependency, type AnyFactory', 'type BindingExtra, type BindingDependency, type AnyFactory').replace('KIND_TAGS, NO_INSTANCES,', 'KIND_TAGS, NO_INSTANCES, lifetimeText,')
s=s.replace('private readonly records = new Map<Binding, StoredRecord>();', 'private readonly records: Map<Binding, StoredRecord>;').replace('private readonly pending = new Map<Binding, Promise<StoredRecord>>();', 'private readonly pending: Map<Binding, Promise<StoredRecord>>;').replace('private readonly disposalOrder: Binding[] = [];', 'private readonly disposalOrder: Binding[];')
s=re.sub(r'  /\*\*[^\n]*\*/\n  private builds\?: Set<ResolutionView>;\n','',s)
s=s.replace('''    this.identityGiven = identity;
    parent?.children.add(this);''', '''    this.identityGiven = identity;
    const target = this;
    this.resolution = new Resolution({
      get identity() { return target.identity; },
      get buildTimeout() { return target.buildTimeout; },
      get recordOwner() { return target.recordOwner; },
      active: () => target.assertActive(),
      route: binding => target.route(binding).resolution,
      selectSingle: (key, staged) => target.locateSingle(key, staged),
      selectMulti: (key, staged) => target.locateMulti(key, staged),
      autoBinding: (token, key) => target.autoBinding(token, key),
      assertSingle: (key, binding, token) => target.assertNotMulti(key, binding, token),
      hasInterceptors: () => target.hasInterceptors(),
      interceptors: () => target.gatherInterceptors(),
      scopeId: binding => (binding.lifetime === ServiceLifetime.Singleton ? target.ownerOf(binding)
        : binding.lifetime === ServiceLifetime.Tenant ? target.nearestScope(ScopeType.Tenant)
        : binding.lifetime === ServiceLifetime.Request ? target.nearestScope(ScopeType.Request) : target)?.identity.id,
      view: (binding, record, parent, context) => new ResolutionView(target, binding, record, parent, null, context.staged, context.suppressInterceptors, target.contextHost, context.completion),
      observe: (from, to) => target.observe(from, to),
      observeResource: (from, to) => target.observeResource(from, to),
      dispose: (record, unreturned) => target.disposeInstance(record, unreturned),
      allBuilds: () => target.ancestors()[0]!.subtree().flatMap(scope => scope.resolution.activeBuilds()),
      exposeIssues,
    });
    this.records = this.resolution.records; this.pending = this.resolution.pending; this.disposalOrder = this.resolution.disposalOrder;
    parent?.children.add(this);''')
pos=s.index('  /** The data the instance') if '  /** The data the instance' in s else s.index('  /** This scope\'s own')
# A declaration, not an accessor, keeps cached get's direct Map fields unchanged.
s=s[:pos]+'''  /** Single-owner resolution/control component; cache fields above alias its exact stores. */
  private readonly resolution: Resolution;
'''+s[pos:]
s=s.replace('target.resolveIn(', 'target.resolution.resolveIn(').replace('target.resolveAllIn(', 'target.resolution.resolveAllIn(')
s=s.replace('this.makeRecord(binding, undefined, this)', 'this.resolution.makeRecord(binding, undefined)').replace('this.makeRecord(binding, instance, this)', 'this.resolution.makeRecord(binding, instance)')
s=s.replace('this.entry(', 'this.resolution.entry(').replace('scope.entry(', 'scope.resolution.entry(').replace('this.recordScope(oldRecord).entry(', 'this.recordScope(oldRecord).resolution.entry(')
s=s.replace("      for (const view of [...(scope.builds ?? [])]) scope.stopBuild(view, new DiError(`Build cancelled: ${tokenName(view.binding.token)}`, 'DI_BUILD_CANCELLED'));", '      scope.resolution.cancel();')
# Thin public orchestration methods preserve the original API surface and synchronous path.
pos=s.index('  /**\n   * The built instance this scope sees')
s=s[:pos]+'''  /** Resolves through the owning explicit resolution pipeline. */
  resolve<T, D = void>(token: InjectionToken<T>, data?: D): Promise<T> { return this.resolution.resolve(token, data); }
  /** Active ambient factory frame, or a fresh external root. */
  private rootCtx(): Resolver { return this.resolution.rootCtx(); }
  /** Internal synchronous-or-asynchronous slot path used by constructor facades. */
  resolveEither(token: InjectionToken): unknown { return this.resolution.resolveEither(token); }
  /** Keeps synchronous graphs synchronous; async graphs retain their normal controlled build. */
  resolveSync<T, D = void>(token: InjectionToken<T>, data?: D): T { return this.resolution.resolveSync(token, data); }
  /** Each caller owns its multi batch; entries still share their individual controlled builds. */
  resolveAll<T, D = void>(token: InjectionToken<T>, data?: D): Promise<T[]> { return this.resolution.resolveAll(token, data); }

'''+s[pos:]
open(p,'w').write(s)
