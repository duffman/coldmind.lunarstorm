from pathlib import Path
import re
# Explicit registration-bound facade: every registration is captured, no global ambient transaction.
f=Path('src/core/cminject.view.ts');s=f.read_text();s=s.replace('  constructor(readonly target: ViewTarget) {}','''  constructor(readonly target: ViewTarget, protected readonly registration?: RegistrationCapture) {}
  protected registering<T, D>(work: () => BindingHandle<T, D>): BindingHandle<T, D> {
    this.registration?.active();
    const handle = work(); this.registration?.capture(handle); return handle;
  }
  protected ordinary(): void { if (this.registration) throw new Error('Module registration facade only permits registration and resolution'); }
''')
# Wrap only explicit registration forwards, keeping overloads and real receiver intact.
for name in ['singleton','transient','scoped','tenant','request','singletonMulti','transientMulti','scopedMulti','tenantMulti','requestMulti','register','registerClass','registerInstance']:
 pattern=re.compile(r'(return )(this\.target\.'+name+r'(?:<[^;]+?>)?\([^;]*?\))(;)',re.M)
 s=pattern.sub(r'\1this.registering(() => \2)\3',s)
# Dangerous mutation/scope operations are rejected before the target is called.
for name in ['load','snapshot','restore','createScope','createTenantScope','createRequestScope','unregister','unregisterBinding','replace','replaceBinding','dispose','disposeWithTracking','clearInstances','clearInstancesWithTracking','clear']:
 pattern=re.compile(r'(^  '+name+r'(?:<[^\n]*?>)?\([^\n]*?\)\s*:\s*[^\n]*?\{)',re.M)
 s=pattern.sub(r'\1 this.ordinary();',s)
s+='''
/** Per-load registration capture; a retained facade becomes invalid after callback settlement. */
export interface RegistrationCapture {
  active(): void;
  capture<T, D>(handle: BindingHandle<T, D>): void;
}
/** Explicit transaction context survives await without changing the target's ambient state. */
export class RegistrationView extends ContainerView {
  constructor(target: ViewTarget, protected readonly host: ContextHost, capture: RegistrationCapture) { super(target, capture); }
  resolutionContext(): Resolver { this.registration!.active(); return this.host.context(); }
  resolveEither(token: InjectionToken): unknown { return this.host.resolveEither(this.resolutionContext(), token); }
  resolve<T, D = void>(token: InjectionToken<T>, data?: D): Promise<T> { return this.host.resolve(this.resolutionContext(), token, data); }
  resolveSync<T, D = void>(token: InjectionToken<T>, data?: D): T { return this.host.resolveSync(this.resolutionContext(), token, data); }
  resolveAll<T, D = void>(token: InjectionToken<T>, data?: D): Promise<T[]> { return this.host.resolveAll(this.resolutionContext(), token, data); }
  get signal(): AbortSignal { return this.target.signal; }
}
''';f.write_text(s)
# Module metadata lives on definitions, so diagnostics/clone/replace don't lose it.
f=Path('src/core/cminject.binding.ts');s=f.read_text().replace('get module(): string | undefined { return this.extra?.module; }','get module(): string | undefined { return this.extra?.module; }\n  set module(value: string | undefined) { (this.extra ??= {}).module = value; }');f.write_text(s)
f=Path('src/core/cminject.host.ts');s=f.read_text().replace('  factoryArgs(container:', '  guardMutation(bindingId: string): void;\n  factoryArgs(container:');f.write_text(s)
f=Path('src/core/cminject.mutation.ts');s=f.read_text();s=s.replace('    const old = this.host.order.find(b => b.id === bindingId);','    this.host.guardMutation(bindingId);\n    const old = this.host.order.find(b => b.id === bindingId);',1)
s=s.replace('async unregisterBinding(bindingId: string): Promise<boolean> {\n    this.host.active();','async unregisterBinding(bindingId: string, restoring = false): Promise<boolean> {\n    if (!restoring) { this.host.active(); this.host.guardMutation(bindingId); }')
f.write_text(s)
f=Path('src/core/cminject.container.ts');s=f.read_text();s="import { ModuleRegistry } from '@coldmind/inject/module';\nimport { RegistrationView } from '@coldmind/inject/internal/view';\n"+s
pos=s.index('/** Whether the container disposes');s=s[:pos]+'''interface CapturedRegistry {
  owner: Container;
  scopes: Array<{ scope: Container; definitions: Binding[]; records: Map<Binding, StoredRecord>; epoch: number }>;
  modules: ReturnType<ModuleRegistry['snapshot']> | undefined;
}
const registrySnapshots = new WeakMap<RegistrySnapshot, CapturedRegistry>();
'''+s[pos:]
pos=s.index('  /** Opaque ownership identity');s=s[:pos]+'''  private moduleRegistry?: ModuleRegistry;
  private readonly moduleLocks = new Set<string>();
  private registryRestoring = false;
  private restoredGeneration = 0;
  private get rootContainer(): Container { return this.parent ? this.parent.rootContainer : this; }
  private assertMutable(): void {
    this.assertActive();
    if (this.rootContainer.registryRestoring) throw new MutationConflictError('Registry restoration is in progress');
  }
  private get modules(): ModuleRegistry {
    const root = this.rootContainer;
    return root.moduleRegistry ??= new ModuleRegistry({
      active: () => root.assertActive(),
      cycleError: path => new DiGraphError([{ kind: 'module-cycle', at: Symbol(path[0]), message: `Module cycle: ${path.join(' -> ')}` }]),
      runRegistration: module => root.registerModule(module),
    });
  }
  load(...modules: ModuleDefinition[]): Promise<ModuleHandle[]> { this.assertMutable(); return this.modules.load(...modules); }
  private async registerModule(module: ModuleDefinition): Promise<readonly BindingHandle[]> {
    this.assertMutable();
    const handles: BindingHandle[] = []; let live = true;
    const facade = new RegistrationView(this, this.contextHost, {
      active: () => { this.assertMutable(); if (!live) throw new LifecycleError('Module registration callback has completed'); },
      capture: handle => {
        const binding = this.order.find(item => item.id === handle.bindingId)!;
        binding.module = module.name; this.moduleLocks.add(binding.id); handles.push(handle);
      },
    });
    try { await module.register(facade); this.assertActive(); return handles; }
    catch (error) {
      const failures: unknown[] = [];
      for (const handle of handles.slice().reverse()) {
        this.moduleLocks.delete(handle.bindingId);
        if (!this.disposed && !this.disposing) try { await handle.unregister(); } catch (failure) { failures.push(failure); }
      }
      if (failures.length) throw new AggregateError([error, ...failures], `Module rollback failed: ${module.name}`);
      throw error;
    } finally { live = false; for (const handle of handles) this.moduleLocks.delete(handle.bindingId); }
  }
  snapshot(): RegistrySnapshot {
    this.assertMutable();
    if (this.subtree().some(scope => scope.pending.size || scope.moduleLocks.size)) throw new MutationConflictError('Cannot snapshot pending builds or module registrations');
    const snapshot = Object.freeze({}) as RegistrySnapshot;
    registrySnapshots.set(snapshot, { owner: this, scopes: this.subtree().map(scope => ({ scope, definitions: [...scope.order], records: new Map(scope.records), epoch: scope.epoch })), modules: this.rootContainer.moduleRegistry?.snapshot() });
    return snapshot;
  }
  async restore(snapshot: RegistrySnapshot): Promise<void> {
    this.assertMutable();
    const captured = registrySnapshots.get(snapshot);
    if (!captured || captured.owner !== this) throw new MutationConflictError('Snapshot belongs to another container');
    const scopes = this.subtree();
    if (scopes.some(scope => scope.pending.size || scope.moduleLocks.size)) throw new MutationConflictError('Cannot restore pending builds or module registrations');
    // Atomic preflight: irreconstructible owned supplied values cannot be brought back.
    for (const state of captured.scopes) if (!state.scope.disposed && !state.scope.disposing) {
      for (const binding of state.definitions) if (binding.kindTag === 'registerInstance' && binding.owner.ownership === 'owned') {
        const record = state.records.get(binding);
        if (!record || record.state !== 'ready' || record.cleanupStarted || state.scope.records.get(binding) !== record) {
          throw new MutationConflictError(`Cannot restore disposed owned supplied instance: ${tokenName(binding.token)}`);
        }
      }
    }
    this.rootContainer.registryRestoring = true;
    const identities = new Map<string, BindingHandle>();
    try {
      for (const scope of scopes.slice().reverse()) {
        const state = captured.scopes.find(item => item.scope === scope);
        const keep = new Set(state?.definitions.filter(binding => scope.order.includes(binding)) ?? []);
        for (const binding of [...scope.order].reverse()) if (!keep.has(binding)) await scope.mutation.unregisterBinding(binding.id, true);
        // Keep only exact live records captured before the test; post-snapshot materializations are disposed.
        for (const record of [...scope.records.values()].reverse()) if (state?.records.get(record.binding) !== record) {
          await scope.disposeRecord(record, true); scope.records.delete(record.binding);
          const index = scope.disposalOrder.indexOf(record.binding); if (index >= 0) scope.disposalOrder.splice(index, 1);
        }
        if (!state) continue;
        for (const binding of state.definitions) {
          if (keep.has(binding)) { identities.set(binding.id, scope.handle(binding)); continue; }
          const restored = createBinding(scope.bindingOwner, binding.token, binding.lifecycle ?? binding.factory, binding.lifetime, binding.kind, binding.kindTag, undefined,
            { deps: [...binding.deps], declared: binding.declared, owner: binding.owner, ctor: binding.ctor, descriptor: binding.descriptor,
              timeoutMs: binding.timeoutMs, generation: binding.generation + ++scope.restoredGeneration, when: binding.when, module: binding.module });
          scope.mutation.install(restored); identities.set(binding.id, restored);
          if (binding.kindTag === 'registerInstance') {
            const original = state.records.get(binding)!;
            const record = scope.resolution.makeRecord(restored, original.data); record.instance = original.instance; record.state = 'ready'; record.produced = true; record.building = false;
            scope.records.set(restored, record); scope.disposalOrder.push(restored);
          }
        }
        scope.epoch = state.epoch; scope.observation.clearRuntime();
      }
      if (captured.modules) this.rootContainer.modules.restore(captured.modules, identities);
      else this.rootContainer.moduleRegistry?.reset();
    } finally { this.rootContainer.registryRestoring = false; }
  }
'''+s[pos:]
s=s.replace('        active: () => target.assertActive(),\n        subtree:', '        active: () => target.assertMutable(),\n        guardMutation: id => { if (target.moduleLocks.has(id)) throw new MutationConflictError(`Module registration owns binding: ${id}`); },\n        subtree:')
# Root disposal invalidates module handles before user cleanup.
s=s.replace('    this.disposing = true;', '    this.disposing = true;\n    if (!this.parent) this.moduleRegistry?.invalidate();',1)
s=s.replace('  validate(): void { this.observation.validate(); }','  validate(options?: ValidationOptions): void { this.observation.validate(options); }')
f.write_text(s)
# Snapshot module ownership without re-running registration bodies; rebind exact handles on restoration.
f=Path('src/core/cminject.module.ts');s=f.read_text();pos=s.index('  /** Acquires one explicit owner');s=s[:pos]+'''  snapshot(): ModuleRegistrySnapshot {
    this.active();
    return [...this.entries.values()].map(entry => ({ module: entry.module, definition: entry.definition, dependencies: [...entry.dependencies].map(item => item.module), explicit: !!entry.explicit, bindings: entry.bindings.map(item => ({ id: item.bindingId!, handle: item })) }));
  }
  reset(): void {
    for (const entry of this.entries.values()) if (entry.explicit) entry.explicit.active = false;
    this.entries.clear();
  }
  restore(snapshot: ModuleRegistrySnapshot, bindings: ReadonlyMap<string, BindingHandle>): void {
    this.reset();
    for (const item of snapshot) {
      const entry = new ModuleEntry(item.module, item.definition, async value => value);
      entry.bindings = item.bindings.map(binding => bindings.get(binding.id) ?? binding.handle);
      this.entries.set(item.module, entry); if (item.explicit) this.explicit(entry);
    }
    for (const item of snapshot) {
      const entry = this.entries.get(item.module)!;
      for (const module of item.dependencies) { const dependency = this.entries.get(module)!; entry.dependencies.add(dependency); dependency.dependents.add(entry); }
    }
  }
'''+s[pos:]
s=s.replace("Pick<BindingHandle, 'unregister'>", "Pick<BindingHandle, 'unregister'> & Partial<Pick<BindingHandle, 'bindingId'>>")
s+='''
/** Internal ownership checkpoint, rebound to fresh registration handles by Container.restore. */
export type ModuleRegistrySnapshot = Array<{ module: ModuleDefinition; definition: DefinitionSnapshot; dependencies: ModuleDefinition[]; explicit: boolean; bindings: Array<{ id: string; handle: ModuleOwnedBinding }> }>;
''';f.write_text(s)
print('Wrote modules, explicit transactional facade and registry snapshot/restore; no checks run.')
