import type { BindingHandle, IContainer, RegisterOptions, ServiceFactory, ServiceLifecycle } from '../../src/core/cminject.contracts.ts';
import type { Constructor, InjectionToken } from '../../src/core/cminject.token.ts';
import type { DependencySlot, DependencyValues } from '../../src/core/cminject.lazy.ts';

/** Dependency values are supplied positionally, with no container/data prefix. */
export type InjectedFactory<T, Slots extends readonly DependencySlot[]> = (...args: DependencyValues<Slots>) => T | Promise<T>;
export type InjectedLifecycle<T, Slots extends readonly DependencySlot[], Factory extends InjectedFactory<T, Slots>> = {
  readonly inject: Slots;
  factory: Factory;
} & Pick<ServiceLifecycle<T>, 'onInit' | 'onDispose' | 'onHealthCheck'>;
/** Capture actual callback parameters before requiring an exact finite tuple length. */
export type ExactFactoryOptions<Slots extends readonly DependencySlot[], Factory extends (...args: never[]) => unknown> =
  number extends Slots['length'] ? [tupleRequired: never] : Parameters<Factory>['length'] extends Slots['length'] ? Slots['length'] extends Parameters<Factory>['length'] ? [options?: RegisterOptions] : [arityMismatch: never] : [arityMismatch: never];

/** Typed dialect first: ordering is required for callback contextual inference. */
export interface TypedRegistration {
  <T, const Slots extends readonly DependencySlot[], Factory extends InjectedFactory<T, Slots>>(token: InjectionToken<T>, lifecycle: InjectedLifecycle<T, Slots, Factory>, ...options: ExactFactoryOptions<Slots, Factory>): BindingHandle<T>;
  <T, TData = void>(token: InjectionToken<T>, factory: ServiceFactory<T, TData> | (ServiceLifecycle<T, TData> & { readonly inject?: never }), options?: RegisterOptions): BindingHandle<T, TData>;
  <T>(ctor: Constructor<T>, options?: RegisterOptions): BindingHandle<T>;
}
export type TypedFactoryContainer = Omit<IContainer, 'singleton' | 'transient' | 'scoped' | 'tenant' | 'request' | 'singletonMulti' | 'transientMulti' | 'scopedMulti' | 'tenantMulti' | 'requestMulti'> & {
  singleton: TypedRegistration;
  transient: TypedRegistration;
  scoped: TypedRegistration;
  tenant: TypedRegistration;
  request: TypedRegistration;
  singletonMulti: TypedRegistration;
  transientMulti: TypedRegistration;
  scopedMulti: TypedRegistration;
  tenantMulti: TypedRegistration;
  requestMulti: TypedRegistration;
};
