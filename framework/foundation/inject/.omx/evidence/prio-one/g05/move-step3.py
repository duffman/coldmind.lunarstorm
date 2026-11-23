import os
for name in ['container','binding','host','async']:
 p=f'src/core/cminject.{name}.ts';dest='../../../.archive/2026-10-27/revisions/g05-step3/packages/foundation/inject/'+p;os.makedirs(os.path.dirname(dest),exist_ok=True)
 if not os.path.exists(dest):open(dest,'w').write(open(p).read())
p='src/core/cminject.container.ts';s=open(p).read()
a=s.index('/** A built (or building) instance');b=s.index('/** A dependency between two bindings',a);shape=s[a:b];s=s[:a]+s[b:]
a=s.index('/** The id of an instance, assigned');b=s.index('/** The resolution path ending',a);ids=s[a:b];s=s[:a]+s[b:]
a=s.index('/** Keeps fallback controllers alive');b=s.index('/** Diagnostic logs keep',a);signal=s[a:b];s=s[:a]+s[b:]
a=s.index('/** A built (or building) instance as the container stores it.');record=s[a:];s=s[:a]
shape=shape.replace('container: Container;', 'container: RecordOwner;')
record=record.replace('class StoredRecord', 'export class StoredRecord').replace('container: Container', 'container: RecordOwner')
ids=ids.replace('function idOf(', 'export function idOf(')
bp='src/core/cminject.binding.ts';bs=open(bp).read().replace('type IContainer,', 'type IContainer, type InstanceRecord,').replace('lookupToken, nextSequence,', 'lookupToken, nextResourceInstanceId, nextSequence,').replace('type { BindingOwner }', 'type { BindingOwner, RecordOwner }').replace("import { stamp }", "import type { DiError } from '@coldmind/inject/src/core/cminject.errors.ts';\nimport { stamp, buildSignal }")
bs+='\n'+shape+ids+record;open(bp,'w').write(bs)
ap='src/core/cminject.async.ts';at=open(ap).read();at+='\n'+signal.replace('function buildSignal(', 'export function buildSignal(');open(ap,'w').write(at)
hp='src/core/cminject.host.ts';ht=open(hp).read();ht="import type { DiError } from '@coldmind/inject/src/core/cminject.errors.ts';\n"+ht+'''\n/** Lifetime-only capabilities retained by stored instances, never the owning container implementation. */
export interface RecordOwner {
  /** Owning scope abort, including for successfully returned transient signals. */
  readonly signal: AbortSignal;
  /** The teardown reason, when this scope has entered disposal. */
  readonly lifetimeReason: DiError | undefined;
}
''';open(hp,'w').write(ht)
s=s.replace('TypedBinding, Handle,', 'StoredRecord, idOf, TypedBinding, Handle,').replace('type { BindingOwner }', 'type { BindingOwner, RecordOwner }').replace('nextResourceInstanceId, ', '')
pos=s.index('/** Internal hierarchy routing');s=s[:pos]+"/** Core-only mapping from record lifetime capabilities to hierarchy owners. */\nconst recordScopes = new WeakMap<RecordOwner, Container>();\n"+s[pos:]
pos=s.index('  /** Definition/handle callbacks')
s=s[:pos]+'''  /** Stable lifetime capabilities for all records owned by this scope. */
  private recordOwnerState?: RecordOwner;
  /** Record helpers observe lifetime without receiving mutable container authority. */
  private get recordOwner(): RecordOwner {
    if (!this.recordOwnerState) {
      const target = this;
      this.recordOwnerState = { get signal() { return target.signal; }, get lifetimeReason() { return target.lifetimeReason; } };
      recordScopes.set(this.recordOwnerState, this);
    }
    return this.recordOwnerState;
  }
  /** Hierarchy translation remains private to container orchestration. */
  private recordScope(record: StoredRecord): Container {
    const scope = recordScopes.get(record.container);
    if (!scope) throw new LifecycleError('Unknown record owner');
    return scope;
  }

'''+s[pos:]
s=s.replace('new StoredRecord(binding, container, data, at)', 'new StoredRecord(binding, container.recordOwner, data, at)').replace('const container = record.container;', 'const container = this.recordScope(record);').replace('stageTargets.push({ scope: record.container,', 'stageTargets.push({ scope: this.recordScope(record),').replace('await oldRecord.container.entry(', 'await this.recordScope(oldRecord).entry(')
open(p,'w').write(s)
