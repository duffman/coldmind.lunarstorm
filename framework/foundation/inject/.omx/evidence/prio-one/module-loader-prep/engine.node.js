// src/core/cminject.errors.ts
class DiError extends Error {
  code;
  constructor(message, code, options) {
    super(message, options);
    this.code = code;
    this.name = new.target.name;
  }
}
class MutationConflictError extends DiError {
  scopeKind;
  constructor(message, scopeKind) {
    super(message, "DI_MUTATION_CONFLICT");
    this.scopeKind = scopeKind;
  }
}

class LifecycleError extends DiError {
  constructor(message) {
    super(message, "DI_LIFECYCLE_ERROR");
  }
}

// .omx/evidence/prio-one/module-loader-prep/engine.ts
class ModuleEntry {
  module;
  definition;
  dependencies = new Set;
  dependents = new Set;
  ready;
  explicit;
  bindings = [];
  removal;
  constructor(module, definition, build) {
    this.module = module;
    this.definition = definition;
    this.ready = Promise.resolve().then(() => build(this));
  }
}

class ModuleRegistry {
  host;
  entries = new Map;
  closed = false;
  constructor(host) {
    this.host = host;
  }
  async load(...modules) {
    this.active();
    const plan = this.prepare(modules);
    const acquired = await Promise.all(modules.map((module) => this.acquire(module, plan)));
    return acquired.map((entry) => {
      const owner = entry.explicit;
      if (!owner?.active)
        throw new LifecycleError(`Module unloaded before load completed: ${entry.definition.name}`);
      return owner.handle;
    });
  }
  invalidate() {
    if (this.closed)
      return;
    this.closed = true;
    for (const entry of this.entries.values()) {
      if (entry.explicit)
        entry.explicit.active = false;
      entry.explicit = undefined;
    }
    this.entries.clear();
  }
  active() {
    if (this.closed)
      throw new LifecycleError("Module registry belongs to a disposed root");
    this.host.active();
  }
  prepare(modules) {
    const snapshots = new Map;
    const path = [];
    const active = new Set;
    const visit = (module) => {
      if (active.has(module)) {
        const start = path.indexOf(module);
        throw this.host.cycleError([...path.slice(start), module].map((item) => item.name));
      }
      if (snapshots.has(module))
        return;
      if (!module || typeof module.name !== "string" || !module.name.trim() || typeof module.register !== "function" || module.imports !== undefined && !Array.isArray(module.imports))
        throw new TypeError("Invalid module definition");
      const current = this.entries.get(module);
      if (current && !current.removal)
        this.assertUnchanged(current);
      const definition = { name: module.name, register: module.register, imports: [...module.imports ?? []] };
      snapshots.set(module, definition);
      active.add(module);
      path.push(module);
      for (const dependency of definition.imports)
        visit(dependency);
      path.pop();
      active.delete(module);
    };
    for (const module of modules)
      visit(module);
    return snapshots;
  }
  assertUnchanged(entry) {
    const module = entry.module;
    const definition = entry.definition;
    const imports = module.imports ?? [];
    if (module.name !== definition.name || module.register !== definition.register || imports.length !== definition.imports.length || imports.some((dependency, index) => dependency !== definition.imports[index])) {
      throw new MutationConflictError(`Cannot mutate a loaded or loading module definition: ${definition.name}`);
    }
  }
  async acquire(module, plan, parent) {
    this.active();
    let entry = this.entries.get(module);
    while (entry?.removal) {
      await entry.removal.catch(() => {
        return;
      });
      this.active();
      entry = this.entries.get(module);
    }
    if (!entry) {
      const definition = plan.get(module);
      if (!definition)
        throw new LifecycleError("Module absent from validated import plan");
      entry = new ModuleEntry(module, definition, (current) => this.build(current, plan));
      this.entries.set(module, entry);
    }
    this.assertUnchanged(entry);
    if (parent) {
      parent.dependencies.add(entry);
      entry.dependents.add(parent);
    } else if (!entry.explicit) {
      this.explicit(entry);
    }
    try {
      return await entry.ready;
    } catch (error) {
      if (parent) {
        parent.dependencies.delete(entry);
        entry.dependents.delete(parent);
      }
      throw error;
    }
  }
  explicit(entry) {
    const owner = { active: true, handle: { unload: () => {
      if (!owner.active)
        return owner.releasing ?? Promise.resolve();
      owner.active = false;
      if (entry.explicit === owner)
        entry.explicit = undefined;
      owner.releasing = this.releaseUnused(entry);
      return owner.releasing;
    } } };
    entry.explicit = owner;
  }
  async build(entry, plan) {
    try {
      for (const dependency of entry.definition.imports)
        await this.acquire(dependency, plan, entry);
      this.active();
      this.assertUnchanged(entry);
      entry.bindings = await this.host.runRegistration(entry.module);
      this.active();
      this.assertUnchanged(entry);
      return entry;
    } catch (error) {
      if (entry.explicit)
        entry.explicit.active = false;
      entry.explicit = undefined;
      const cleanupErrors = [];
      if (!this.closed) {
        await this.removeBindings(entry, cleanupErrors);
        await this.releaseDependencies(entry, cleanupErrors);
      }
      if (this.entries.get(entry.module) === entry)
        this.entries.delete(entry.module);
      if (cleanupErrors.length)
        throw new AggregateError([error, ...cleanupErrors], `Module load failed and rollback reported errors: ${entry.definition.name}`);
      throw error;
    }
  }
  async removeBindings(entry, errors) {
    const bindings = entry.bindings;
    entry.bindings = [];
    for (const binding of [...bindings].reverse()) {
      if (this.closed)
        break;
      try {
        await binding.unregister();
      } catch (error) {
        errors.push(error);
      }
    }
  }
  async releaseDependencies(entry, errors) {
    const dependencies = [...entry.dependencies].reverse();
    entry.dependencies.clear();
    for (const dependency of dependencies) {
      dependency.dependents.delete(entry);
      try {
        await this.releaseUnused(dependency);
      } catch (error) {
        errors.push(error);
      }
    }
  }
  async releaseUnused(entry) {
    if (this.closed || entry.explicit || entry.dependents.size)
      return;
    if (entry.removal)
      return entry.removal;
    try {
      await entry.ready;
    } catch {
      return;
    }
    if (this.closed || entry.explicit || entry.dependents.size || this.entries.get(entry.module) !== entry)
      return;
    if (entry.removal)
      return entry.removal;
    entry.removal = Promise.resolve().then(async () => {
      const errors = [];
      await this.removeBindings(entry, errors);
      await this.releaseDependencies(entry, errors);
      if (this.entries.get(entry.module) === entry)
        this.entries.delete(entry.module);
      if (errors.length)
        throw new AggregateError(errors, `Module unload failed: ${entry.definition.name}`);
    });
    return entry.removal;
  }
}
export {
  ModuleRegistry
};
