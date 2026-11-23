export type ServiceType<T = object> = new () => T;
const serviceTypes = new WeakSet<Function>();
const resolvers = new WeakMap<object, ServiceContainer>();

export function Service<T extends Function>(target: T): T {
  serviceTypes.add(target);
  return target;
}

export function Inject<T extends object>(token: ServiceType<T>): PropertyDecorator {
  return (target, key) => {
    Object.defineProperty(target, key, {
      configurable: true,
      get(this: object) {
        const resolver = resolvers.get(this);
        if (!resolver) throw new Error('Injected services are available after application ownership is established.');
        return resolver.get(token);
      },
    });
  };
}

export function connectServices(instance: object, services: ServiceContainer): void {
  resolvers.set(instance, services);
}

export class ServiceContainer {
  private readonly factories = new Map<ServiceType, () => object>();
  private readonly instances = new Map<ServiceType, object>();
  private readonly constructing = new Set<ServiceType>();
  private released = false;

  registerFactory<T extends object>(token: ServiceType<T>, factory: () => T): void {
    if (this.released || this.instances.has(token)) throw new Error('Service registration is closed for this token.');
    this.factories.set(token, factory);
  }

  get<T extends object>(token: ServiceType<T>): T {
    if (this.released) throw new Error('Service container has been disposed.');
    const existing = this.instances.get(token);
    if (existing) return existing as T;
    if (this.constructing.has(token)) throw new Error('Circular service dependency.');
    if (!this.factories.has(token) && !serviceTypes.has(token)) throw new Error('Injected class must be decorated with @Service or registered explicitly.');
    this.constructing.add(token);
    try {
      const instance = this.factories.get(token)?.() ?? new token();
      connectServices(instance, this);
      this.instances.set(token, instance);
      return instance as T;
    } finally {
      this.constructing.delete(token);
    }
  }

  dispose(): void {
    if (this.released) return;
    this.released = true;
    const errors: unknown[] = [];
    for (const instance of [...this.instances.values()].reverse()) {
      try {
        if ('dispose' in instance && typeof instance.dispose === 'function') instance.dispose();
      } catch (error) { errors.push(error); }
    }
    this.instances.clear();
    this.factories.clear();
    if (errors.length) throw new AggregateError(errors, 'Service disposal failed.');
  }
}
