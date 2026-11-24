import type { BindingHandle, IContainer, ServiceFactory, ServiceLifecycle, ServiceLifetime } from '@coldmind/inject/contracts';
import type { InjectionToken } from '@coldmind/inject/token';

/**
 * Swaps the factory for a token at runtime, for hot module replacement.
 * Live instances are rebuilt so the new factory takes effect; the old registration is removed.
 */
export function hotReplace<T, D = void>(
  container: IContainer,
  token: InjectionToken<T>,
  factory: ServiceFactory<T, D> | ServiceLifecycle<T, D>,
  lifetime: ServiceLifetime,
): Promise<BindingHandle<T, D>> {
  return container.replace(token, factory, lifetime, { rebuild: true, retainPrevious: false });
}
