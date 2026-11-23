import os
p='src/core/cminject.container.ts';s=open(p).read()
for name in ['cminject.container.ts','cminject.async.ts']:
 f='src/core/'+name;dest='../../../.archive/2026-10-27/revisions/g05-step2/packages/foundation/inject/'+f;os.makedirs(os.path.dirname(dest),exist_ok=True)
 if not os.path.exists(dest):open(dest,'w').write(open(f).read())
# Shared high-resolution clock remains one module-level owner.
a=s.index('/** High-resolution timestamp');b=s.index('/** Lifetime names',a);clock=s[a:b];s=s[:a]+s[b:]
clock=clock.replace('const now =','export const now =').replace('function stamp()', 'export function stamp()')
afile='src/core/cminject.async.ts';at=open(afile).read();pos=at.index('/** Only the instance slot');at=at[:pos]+clock+at[pos:];open(afile,'w').write(at)
# Binding-only definitions and their shared constants.
a=s.index('/** Owner policy');b=s.index('/** Message of the',a);constants=s[a:b];s=s[:a]+s[b:]
a=s.index('/** A factory with its data type erased');b=s.index('/** Runtime construction check',a);factory=s[a:b];s=s[:a]+s[b:]
a=s.index('/** Operational registry identity');b=s.index('/** A built (or building) instance',a);types=s[a:b];s=s[:a]+s[b:]
a=s.index('/** No retained instances.');b=s.index('/** A built (or building) instance',a);classes=s[a:b];s=s[:a]+s[b:]
a=s.index('/** Binding fields only some bindings');extra=s[a:];s=s[:a]
a=s.index('/** Live multi-definition counts');b=s.index('/** Registrations of one token',a);multi=s[a:b];s=s[:a]+s[b:]
# Stable binding owner capabilities, never a Container runtime instance.
owner='''import type { BindingHandle, ReplacementOptions, ScopeIdentity, ServiceFactory, ServiceLifecycle, ServiceLifetime } from '@coldmind/inject/src/core/cminject.contracts.ts';

/** Capabilities retained by definitions and public handles, independent of container implementation state. */
export interface BindingOwner {
  /** Stable owning scope identity. */
  readonly identity: ScopeIdentity;
  /** Removes exactly the selected registration. */
  unregisterBinding(bindingId: string): Promise<boolean>;
  /** Replaces exactly the selected typed registration. */
  replaceBinding<T, D = void>(bindingId: string, factory: ServiceFactory<T, D> | ServiceLifecycle<T, D>, lifetime?: ServiceLifetime, options?: ReplacementOptions): Promise<BindingHandle<T, D>>;
}
'''
open('src/core/cminject.host.ts','w').write(owner)
text="""import { ServiceLifetime, type BindingDescriptor, type BindingHandle, type ClassDescriptor, type DependencyEdge, type ReplacementOptions, type ServiceFactory, type ServiceLifecycle } from '@coldmind/inject/src/core/cminject.contracts.ts';
import { type Constructor, type InjectionToken, lookupToken, nextSequence, releaseTokenKey, retainTokenKey, tokenName } from '@coldmind/inject/src/core/cminject.token.ts';
import type { BindingOwner } from '@coldmind/inject/src/core/cminject.host.ts';
import { stamp } from '@coldmind/inject/src/core/cminject.async.ts';

"""+factory+types+constants+multi+"/** Whether the token has any live multi definition across roots. */\nexport const hasMultiBinding = (key: symbol): boolean => multiKeys.has(key);\n"+classes+extra
text=text.replace('ownerContainer: Container','ownerContainer: BindingOwner').replace('private readonly container: Container','private readonly container: BindingOwner')
for old,new in [('type AnyFactory','export type AnyFactory'),('type Binding =','export type Binding ='),('interface BindingShape','export interface BindingShape'),('interface BindingExtra','export interface BindingExtra'),('class Handle<','export class Handle<'),('class TypedBinding<','export class TypedBinding<'),('const KIND_TAGS','export const KIND_TAGS'),('const NO_INSTANCES','export const NO_INSTANCES')]:text=text.replace(old,new)
open('src/core/cminject.binding.ts','w').write(text)
s="import { TypedBinding, Handle, KIND_TAGS, NO_INSTANCES, hasMultiBinding, type Binding, type BindingExtra } from '@coldmind/inject/src/core/cminject.binding.ts';\nimport type { BindingOwner } from '@coldmind/inject/src/core/cminject.host.ts';\n"+s
s=s.replace('chain as then, each,','now, stamp, chain as then, each,').replace('nextResourceInstanceId, nextSequence, nextScopeId','nextResourceInstanceId, nextScopeId').replace('new TypedBinding<T, D>(this,','new TypedBinding<T, D>(this.bindingOwner,').replace('new Handle<T, D>(this,','new Handle<T, D>(this.bindingOwner,').replace('if (!multiKeys.has(key))','if (!hasMultiBinding(key))')
# The core alone translates opaque owner identities into its own hierarchy; helpers cannot access the mutable Container.
pos=s.index('/** A class found') if '/** A class found' in s else s.index('/** Progress of one instance')
s=s[:pos]+"/** Internal hierarchy routing for opaque definition-owner capabilities. */\nconst ownerScopes = new WeakMap<BindingOwner, Container>();\n"+s[pos:]
pos=s.index('  /** Whether resolving an unregistered class')
s=s[:pos]+'''  /** Definition/handle callbacks are bound once; no mutable container escapes into binding helpers. */
  private bindingOwnerState?: BindingOwner;
  /** Stable narrow mutation capabilities used by definitions and retained handles. */
  private get bindingOwner(): BindingOwner {
    if (!this.bindingOwnerState) {
      const target = this;
      this.bindingOwnerState = {
        get identity() { return target.identity; },
        unregisterBinding: id => target.unregisterBinding(id),
        replaceBinding: (id, factory, lifetime, options) => target.replaceBinding(id, factory, lifetime, options),
      };
      ownerScopes.set(this.bindingOwnerState, this);
    }
    return this.bindingOwnerState;
  }
  /** Translates an opaque owner capability only inside container orchestration. */
  private ownerOf(binding: Binding): Container {
    const owner = ownerScopes.get(binding.ownerContainer);
    if (!owner) throw new LifecycleError('Unknown registration owner');
    return owner;
  }

'''+s[pos:]
s=s.replace('case ServiceLifetime.Singleton: return binding.ownerContainer;', 'case ServiceLifetime.Singleton: return this.ownerOf(binding);').replace('c === binding.ownerContainer','c.bindingOwnerState === binding.ownerContainer').replace('binding.lifetime === ServiceLifetime.Singleton ? binding.ownerContainer', 'binding.lifetime === ServiceLifetime.Singleton ? this.ownerOf(binding)').replace('r.binding.ownerContainer === this', 'r.binding.ownerContainer === this.bindingOwnerState').replace('binding.ownerContainer === this', 'binding.ownerContainer === this.bindingOwnerState').replace('b.ownerContainer.ancestors()', 'this.ownerOf(b).ancestors()').replace('b.ownerContainer.locateSingle(', 'this.ownerOf(b).locateSingle(').replace('? [entry.ownerContainer] :', '? [this.ownerOf(entry)] :')
open(p,'w').write(s)
