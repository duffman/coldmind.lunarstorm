import type { Control } from './controls';
import type { View } from './view';

export type ControlType<T extends Control = Control> = new (...args: any[]) => T;
const viewClasses = new WeakSet<Function>();
const actionFunctions = new WeakSet<Function>();

export function ViewClass<T extends Function>(target: T): T {
  viewClasses.add(target);
  return target;
}
export function isViewClass(type: Function): boolean { return viewClasses.has(type); }

/** A constructor token makes the connection available without reflect-metadata. */
export function ViewOutlet<T extends Control>(type: ControlType<T>): PropertyDecorator {
  return (prototype, key) => {
    Object.defineProperty(prototype, key, {
      configurable: true,
      get(this: View): T { return this.resolveOutlet(key, type); },
    });
  };
}

/** Bind once to the owning instance; JSX can pass the method directly. */
export function ViewAction(_prototype: object, key: string | symbol, descriptor: PropertyDescriptor): void {
  const method: unknown = descriptor.value;
  if (typeof method !== 'function') throw new Error('@ViewAction requires a method.');
  const bound = new WeakMap<object, (...args: unknown[]) => Promise<unknown>>();
  delete descriptor.value;
  delete descriptor.writable;
  descriptor.get = function(this: View) {
    let action = bound.get(this);
    if (!action) {
      action = (...args: unknown[]) => {
        if (this.disposed) return Promise.resolve();
        const completion = new Promise<unknown>((resolve, reject) => {
          this.application.dispatch(() => {
            if (this.disposed) { resolve(undefined); return; }
            try { resolve(method.apply(this, args)); }
            catch (error) { reject(error); }
          });
        });
        // Event sources do not await action results. Observe failures centrally
        // while preserving the original promise for explicit callers.
        void completion.catch(error => {
          this.application.dispatch(() => {
            if (!this.disposed) this.application.reportError(error);
          });
        });
        return completion;
      };
      actionFunctions.add(action);
      bound.set(this, action);
    }
    return action;
  };
}
export function isViewAction(value: unknown): value is (...args: any[]) => void {
  return typeof value === 'function' && actionFunctions.has(value);
}
