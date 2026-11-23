import type { BindingHandle } from '@coldmind/inject/src/core/cminject.contracts.ts';
import type { ModuleDefinition, ModuleHandle } from '@coldmind/inject/src/core/cminject.module.ts';
import { LifecycleError, MutationConflictError } from '@coldmind/inject/src/core/cminject.errors.ts';

/** The registry needs only exact-identity removal, not arbitrary replacement authority. */
export type ModuleOwnedBinding = Pick<BindingHandle, 'unregister'>;
/** Container integration owns registration capture, rollback and error construction. */
export interface ModuleRegistryHost {
  active(): void;
  /** Must roll back its own captures before rejecting; invokes the definition synchronously before its first await. */
  runRegistration(module: ModuleDefinition): Promise<readonly ModuleOwnedBinding[]>;
  cycleError(path: readonly string[]): Error;
}
interface DefinitionSnapshot {
  readonly name: string;
  readonly register: ModuleDefinition['register'];
  readonly imports: readonly ModuleDefinition[];
}
interface ExplicitOwner {
  readonly handle: ModuleHandle;
  active: boolean;
  releasing?: Promise<void>;
}
class ModuleEntry {
  readonly dependencies = new Set<ModuleEntry>();
  readonly dependents = new Set<ModuleEntry>();
  readonly ready: Promise<ModuleEntry>;
  explicit?: ExplicitOwner;
  bindings: readonly ModuleOwnedBinding[] = [];
  removal?: Promise<void>;
  constructor(readonly module: ModuleDefinition, readonly definition: DefinitionSnapshot, build: (entry: ModuleEntry) => Promise<ModuleEntry>) {
    // Publish the entry and ownership before registration can execute user code.
    this.ready = Promise.resolve().then(() => build(this));
  }
}

/** One root's identity registry; ordinary registration transactions remain the host's responsibility. */
export class ModuleRegistry {
  private readonly entries = new Map<ModuleDefinition, ModuleEntry>();
  private closed = false;
  constructor(private readonly host: ModuleRegistryHost) {}

  /** Acquires one explicit owner per identity, regardless of repeated or concurrent calls. */
  async load(...modules: readonly ModuleDefinition[]): Promise<ModuleHandle[]> {
    this.active();
    const plan = this.prepare(modules);
    // Each input is an independent explicit load, not an undocumented atomic batch transaction.
    const acquired = await Promise.all(modules.map(module => this.acquire(module, plan)));
    return acquired.map(entry => {
      const owner = entry.explicit;
      if (!owner?.active) throw new LifecycleError(`Module unloaded before load completed: ${entry.definition.name}`);
      return owner.handle;
    });
  }

  /** Root disposal owns all binding teardown. Pending host transactions must reject/rollback on root shutdown. */
  invalidate(): void {
    if (this.closed) return;
    this.closed = true;
    for (const entry of this.entries.values()) {
      if (entry.explicit) entry.explicit.active = false;
      entry.explicit = undefined;
    }
    this.entries.clear();
  }

  private active(): void {
    if (this.closed) throw new LifecycleError('Module registry belongs to a disposed root');
    this.host.active();
  }

  /** Freeze the graph for this attempt, diagnosing cycles before creating waits-for edges. */
  private prepare(modules: readonly ModuleDefinition[]): Map<ModuleDefinition, DefinitionSnapshot> {
    const snapshots = new Map<ModuleDefinition, DefinitionSnapshot>();
    const path: ModuleDefinition[] = [];
    const active = new Set<ModuleDefinition>();
    const visit = (module: ModuleDefinition): void => {
      if (active.has(module)) {
        const start = path.indexOf(module);
        throw this.host.cycleError([...path.slice(start), module].map(item => item.name));
      }
      if (snapshots.has(module)) return;
      if (!module || typeof module.name !== 'string' || !module.name.trim() || typeof module.register !== 'function'
        || (module.imports !== undefined && !Array.isArray(module.imports))) throw new TypeError('Invalid module definition');
      const current = this.entries.get(module);
      if (current && !current.removal) this.assertUnchanged(current);
      const definition = { name: module.name, register: module.register, imports: [...(module.imports ?? [])] };
      snapshots.set(module, definition);
      active.add(module); path.push(module);
      for (const dependency of definition.imports) visit(dependency);
      path.pop(); active.delete(module);
    };
    for (const module of modules) visit(module);
    return snapshots;
  }

  private assertUnchanged(entry: ModuleEntry): void {
    const module = entry.module;
    const definition = entry.definition;
    const imports = module.imports ?? [];
    if (module.name !== definition.name || module.register !== definition.register || imports.length !== definition.imports.length
      || imports.some((dependency, index) => dependency !== definition.imports[index])) {
      throw new MutationConflictError(`Cannot mutate a loaded or loading module definition: ${definition.name}`);
    }
  }

  private async acquire(module: ModuleDefinition, plan: ReadonlyMap<ModuleDefinition, DefinitionSnapshot>, parent?: ModuleEntry): Promise<ModuleEntry> {
    this.active();
    let entry = this.entries.get(module);
    while (entry?.removal) {
      // A previous unload's error belongs to its caller; a new generation may retry after teardown finishes.
      await entry.removal.catch(() => undefined);
      this.active(); entry = this.entries.get(module);
    }
    if (!entry) {
      const definition = plan.get(module);
      if (!definition) throw new LifecycleError('Module absent from validated import plan');
      entry = new ModuleEntry(module, definition, current => this.build(current, plan));
      this.entries.set(module, entry);
    }
    this.assertUnchanged(entry);
    if (parent) {
      parent.dependencies.add(entry); entry.dependents.add(parent);
    } else if (!entry.explicit) {
      this.explicit(entry);
    }
    try { return await entry.ready; }
    catch (error) {
      if (parent) { parent.dependencies.delete(entry); entry.dependents.delete(parent); }
      throw error;
    }
  }

  private explicit(entry: ModuleEntry): void {
    const owner: ExplicitOwner = { active: true, handle: { unload: () => {
      if (!owner.active) return owner.releasing ?? Promise.resolve();
      owner.active = false;
      if (entry.explicit === owner) entry.explicit = undefined;
      owner.releasing = this.releaseUnused(entry);
      return owner.releasing;
    } } };
    entry.explicit = owner;
  }

  private async build(entry: ModuleEntry, plan: ReadonlyMap<ModuleDefinition, DefinitionSnapshot>): Promise<ModuleEntry> {
    try {
      for (const dependency of entry.definition.imports) await this.acquire(dependency, plan, entry);
      this.active(); this.assertUnchanged(entry);
      entry.bindings = await this.host.runRegistration(entry.module);
      this.active(); this.assertUnchanged(entry);
      return entry;
    } catch (error) {
      if (entry.explicit) entry.explicit.active = false;
      entry.explicit = undefined;
      const cleanupErrors: unknown[] = [];
      if (!this.closed) {
        await this.removeBindings(entry, cleanupErrors);
        await this.releaseDependencies(entry, cleanupErrors);
      }
      if (this.entries.get(entry.module) === entry) this.entries.delete(entry.module);
      if (cleanupErrors.length) throw new AggregateError([error, ...cleanupErrors], `Module load failed and rollback reported errors: ${entry.definition.name}`);
      throw error;
    }
  }

  private async removeBindings(entry: ModuleEntry, errors: unknown[]): Promise<void> {
    const bindings = entry.bindings;
    entry.bindings = [];
    for (const binding of [...bindings].reverse()) {
      if (this.closed) break; // Root teardown, not this handle, now owns untouched bindings.
      try { await binding.unregister(); } catch (error) { errors.push(error); }
    }
  }

  private async releaseDependencies(entry: ModuleEntry, errors: unknown[]): Promise<void> {
    const dependencies = [...entry.dependencies].reverse();
    entry.dependencies.clear();
    for (const dependency of dependencies) {
      dependency.dependents.delete(entry);
      try { await this.releaseUnused(dependency); } catch (error) { errors.push(error); }
    }
  }

  private async releaseUnused(entry: ModuleEntry): Promise<void> {
    if (this.closed || entry.explicit || entry.dependents.size) return;
    if (entry.removal) return entry.removal;
    // Imported registration may still be running while its former parent fails.
    try { await entry.ready; } catch { return; }
    if (this.closed || entry.explicit || entry.dependents.size || this.entries.get(entry.module) !== entry) return;
    if (entry.removal) return entry.removal;
    entry.removal = Promise.resolve().then(async () => {
      const errors: unknown[] = [];
      await this.removeBindings(entry, errors);
      await this.releaseDependencies(entry, errors);
      if (this.entries.get(entry.module) === entry) this.entries.delete(entry.module);
      if (errors.length) throw new AggregateError(errors, `Module unload failed: ${entry.definition.name}`);
    });
    return entry.removal;
  }
}
