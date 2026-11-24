import type { IContainer } from '@coldmind/inject/contracts';

/** Named compile-time capabilities; declarations describe registrations rather than inspect their bodies. */
interface ModuleCapabilities<Provides extends object, Requires extends object> {
  /** Services this module promises to provide. */
  readonly provides: Provides;
  /** Services this module requires from its composition. */
  readonly requires: Requires;
}
/** Type-only capability metadata; module values carry no extra runtime field. */
declare const moduleCapabilities: unique symbol;
/** A registration unit; imports are traversed by the container loader, not by its definition. */
export interface ModuleDefinition {
  /** Diagnostic name; module identity is the definition object, not this name. */
  readonly name: string;
  /** Imported definitions, retained by identity so cycles can be diagnosed at load time. */
  readonly imports?: readonly ModuleDefinition[];
  /** Registers this module through the loader's transaction-bound container. */
  register(container: IContainer): void | Promise<void>;
}
/** A module with optional named capability declarations and precise imported definitions. */
export interface Module<Provides extends object = {}, Requires extends object = {}, Imports extends readonly ModuleDefinition[] = readonly ModuleDefinition[], Name extends string = string> extends ModuleDefinition {
  /** Imported definitions; inferred tuples allow composition to check their capabilities too. */
  readonly imports?: Imports;
  /** Diagnostic name retained as a literal when inferred. */
  readonly name: Name;
  /** Phantom declarations; they do not verify the implementation of register. */
  readonly [moduleCapabilities]?: ModuleCapabilities<Provides, Requires>;
}
/** A loaded module's lifetime handle; the container owns the concrete implementation. */
export interface ModuleHandle {
  /** Unloads this module's owned registrations and instances. Idempotent. */
  unload(): Promise<void>;
}
/** A definition's known imports, or an empty tuple when none were supplied. */
type ImportsOf<Definition> = 'imports' extends keyof Definition ? NonNullable<Definition['imports']> extends infer Imports extends readonly ModuleDefinition[] ? Imports : readonly [] : readonly [];
/** Capability metadata exists only on definitions returned with a typed declaration. */
type CapabilitiesOf<Definition> = typeof moduleCapabilities extends keyof Definition ? NonNullable<Definition[typeof moduleCapabilities]> : never;
/** Explicit capabilities provided by a definition, without inferring claims from register. */
export type ModuleProvides<Definition> = Definition extends unknown ? [CapabilitiesOf<Definition>] extends [never] ? {} : CapabilitiesOf<Definition> extends ModuleCapabilities<infer Provides, object> ? Provides : {} : never;
/** Explicit capabilities required by a definition. */
export type ModuleRequires<Definition> = Definition extends unknown ? [CapabilitiesOf<Definition>] extends [never] ? {} : CapabilitiesOf<Definition> extends ModuleCapabilities<object, infer Requires> ? Requires : {} : never;
/** All statically known definitions, stopping at repeated structural identities. */
type ModuleTree<Definition extends ModuleDefinition, Seen = never> = Definition extends Seen ? never : Definition | ModuleTree<ImportsOf<Definition>[number], Seen | Definition>;
/** Names whose imports cannot be statically certified, including recursive type graphs. */
type UncheckedImports<Definition extends ModuleDefinition, Seen = never> = Definition extends Seen ? Definition['name'] : number extends ImportsOf<Definition>['length'] ? Definition['name'] : UncheckedImports<ImportsOf<Definition>[number], Seen | Definition>;
/** Keys present in any member of an object union. */
type UnionKeys<Union> = Union extends object ? keyof Union : never;
/** Values offered by any provider of a named capability; incompatible duplicates remain a union, never an intersection. */
type UnionValue<Union, Key extends PropertyKey> = Union extends object ? Key extends keyof Union ? Union[Key] : never : never;
/** Capability maps declared by every reachable definition. */
type ProvidedUnion<Definitions extends readonly ModuleDefinition[]> = ModuleProvides<ModuleTree<Definitions[number]>>;
/** Requirement maps declared by every reachable definition. */
type RequiredUnion<Definitions extends readonly ModuleDefinition[]> = ModuleRequires<ModuleTree<Definitions[number]>>;
/** The complete offered map, retaining every provider's value type for duplicate names. */
export type ComposedModuleProvides<Definitions extends readonly ModuleDefinition[]> = { [Key in UnionKeys<ProvidedUnion<Definitions>>]: UnionValue<ProvidedUnion<Definitions>, Key> };
/** Requirement names with no compatible provider; every possible provider must meet each declared requirement. */
type Unsatisfied<Provides extends object, Requires> = Requires extends object ? { [Key in keyof Requires]-?: Key extends keyof Provides ? [Provides[Key]] extends [never] ? Key : [Provides[Key]] extends [Requires[Key]] ? never : Key : Key }[keyof Requires] : never;
/** Required types shown in the compiler diagnostic for missing or incompatible named services. */
export type ModuleMissingCapabilities<Definitions extends readonly ModuleDefinition[]> = { [Key in Unsatisfied<ComposedModuleProvides<Definitions>, RequiredUnion<Definitions>>]: UnionValue<RequiredUnion<Definitions>, Key> };
/** An unsatisfied composition names its missing or incompatible capabilities in the compiler error. */
export interface ModuleCompositionError<Capabilities extends object> {
  /** Missing or incompatible named service contracts. */
  readonly missingCapabilities: Capabilities;
}
/** Static completeness requires finite tuples; runtime-defined imports remain supported by ordinary module loading. */
type CompositionCheck<Definitions extends readonly ModuleDefinition[]> = number extends Definitions['length'] ? { readonly moduleTupleRequired: true } : [UncheckedImports<Definitions[number]>] extends [never] ? keyof ModuleMissingCapabilities<Definitions> extends never ? unknown : ModuleCompositionError<ModuleMissingCapabilities<Definitions>> : { readonly uncheckedModuleImports: UncheckedImports<Definitions[number]> };
/** Declares named capabilities without changing the runtime module definition. */
interface ModuleDeclaration<Provides extends object, Requires extends object> {
  /** Captures the definition's literal name and import tuple. */
  <const Definition extends ModuleDefinition>(definition: Definition): Module<Provides, Requires, ImportsOf<Definition>, Definition['name']>;
}
/** Declares an opt-in named capability map, then accepts the same runtime definition. */
export function defineModule<Provides extends object = {}, Requires extends object = {}>(): ModuleDeclaration<Provides, Requires>;
/** Defines an ordinary runtime module without making capability claims. */
export function defineModule<const Definition extends ModuleDefinition>(definition: Definition): Module<{}, {}, ImportsOf<Definition>, Definition['name']>;
export function defineModule(definition?: ModuleDefinition): ModuleDefinition | ((definition: ModuleDefinition) => ModuleDefinition) {
  return definition === undefined ? validateDefinition : validateDefinition(definition);
}
/** Preserves definition, import-array and registration identities; the loader performs graph validation. */
function validateDefinition(definition: ModuleDefinition): ModuleDefinition {
  if (!definition || typeof definition !== 'object' || typeof definition.name !== 'string' || !definition.name.trim()) throw new TypeError('Module name required');
  if (typeof definition.register !== 'function') throw new TypeError(`Module register required: ${definition.name}`);
  if (definition.imports !== undefined && !Array.isArray(definition.imports)) throw new TypeError(`Module imports must be an array: ${definition.name}`);
  return definition;
}
/** Composes statically known modules without executing registration or loading resources. */
export function composeModules<const Definitions extends readonly ModuleDefinition[]>(...modules: Definitions & CompositionCheck<Definitions>): Module<ComposedModuleProvides<Definitions>, {}, Definitions> {
  for (const module of modules) validateDefinition(module);
  return { name: modules.map(module => module.name).join('+') || 'composition', imports: modules, register: () => undefined };
}

import type { BindingHandle } from '@coldmind/inject/contracts';
import { LifecycleError, MutationConflictError } from '@coldmind/inject/errors';

/** The registry needs only exact-identity removal, not arbitrary replacement authority. */
export type ModuleOwnedBinding = Pick<BindingHandle, 'unregister'> & Partial<Pick<BindingHandle, 'bindingId'>>;
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
  loaded = false;
  explicit?: ExplicitOwner;
  bindings: readonly ModuleOwnedBinding[] = [];
  removal?: Promise<void>;
  constructor(readonly module: ModuleDefinition, readonly definition: DefinitionSnapshot, build: (entry: ModuleEntry) => Promise<ModuleEntry>) {
    // Publish the entry and ownership before registration can execute user code.
    this.ready = Promise.resolve().then(() => build(this)).then(entry => { this.loaded = true; return entry; });
  }
}

/** One root's identity registry; ordinary registration transactions remain the host's responsibility. */
export class ModuleRegistry {
  private readonly entries = new Map<ModuleDefinition, ModuleEntry>();
  private closed = false;
  private loading = 0;
  private readonly stopped: Promise<never>;
  private rejectStopped!: (error: Error) => void;
  constructor(private readonly host: ModuleRegistryHost) {
    this.stopped = new Promise((_, reject) => { this.rejectStopped = reject; });
    void this.stopped.catch(() => undefined);
  }

  /** Keeps module ownership attached to successful replacement generations. */
  remapBindings(bindings: ReadonlyMap<string, ModuleOwnedBinding>): void {
    for (const entry of this.entries.values()) entry.bindings = entry.bindings.map(binding => binding.bindingId ? bindings.get(binding.bindingId) ?? binding : binding);
  }
  assertIdle(): void {
    this.active();
    if (this.loading || [...this.entries.values()].some(entry => !entry.loaded || !!entry.removal)) throw new MutationConflictError('Cannot snapshot or restore pending module mutations');
  }
  snapshot(): ModuleRegistrySnapshot {
    this.active();
    this.assertIdle();
    return [...this.entries.values()].map(entry => ({ module: entry.module, definition: entry.definition, dependencies: [...entry.dependencies].map(item => item.module), explicit: !!entry.explicit, bindings: entry.bindings.map(item => ({ id: item.bindingId!, handle: item })) }));
  }
  reset(): void {
    for (const entry of this.entries.values()) if (entry.explicit) entry.explicit.active = false;
    this.entries.clear();
  }
  restore(snapshot: ModuleRegistrySnapshot, bindings: ReadonlyMap<string, ModuleOwnedBinding>): void {
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
  /** Acquires one explicit owner per identity, regardless of repeated or concurrent calls. */
  async load(...modules: readonly ModuleDefinition[]): Promise<ModuleHandle[]> {
    this.active();
    const plan = this.prepare(modules);
    // Each input is an independent explicit load, not an undocumented atomic batch transaction.
    this.loading++;
    let acquired: ModuleEntry[];
    try { acquired = await Promise.all(modules.map(module => this.acquire(module, plan))); } finally { this.loading--; }
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
    this.rejectStopped(new LifecycleError('Module root disposed'));
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
    try { return await Promise.race([entry.ready, this.stopped]); }
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

/** Internal ownership checkpoint, rebound to fresh registration handles by Container.restore. */
export type ModuleRegistrySnapshot = Array<{ module: ModuleDefinition; definition: DefinitionSnapshot; dependencies: ModuleDefinition[]; explicit: boolean; bindings: Array<{ id: string; handle: ModuleOwnedBinding }> }>;
