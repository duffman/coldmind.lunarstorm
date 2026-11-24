import { Action } from '@coldmind/wire/src/core/action.ts';
import { ActionLifecycle } from '@coldmind/wire/src/component/action.lifecycle.ts';
import type { ActionLifecycleOptions } from '@coldmind/wire/src/component/action.lifecycle.ts';
import type { ActionAsync, ActionCleanup } from '@coldmind/wire/src/core/action.operation.ts';
import type { ActionTelemetry } from '@coldmind/wire/src/telemetry/action.telemetry.ts';

const portMarker: unique symbol = Symbol('ActionWire.port');
declare const portValue: unique symbol;

/** A port declaration carrying `T`; every component instance turns it into its own `Action<T>`. */
export interface ActionPort<T> {
  readonly [portMarker]: true;
  readonly [portValue]?: T;
}
/** Declares a port carrying `T` in a component definition's `inputs` or `outputs`. */
export function actionPort<T>(): ActionPort<T> { return Object.freeze({ [portMarker]: true as const }); }
/** The live channels of one component instance, one `Action` per port. */
export type ActionPorts<Values extends object> = { readonly [K in keyof Values]: Action<Values[K]> };
/** The port declarations of a component, one `actionPort<T>()` per port. */
export type ActionPortDefinitions<Values extends object> = { readonly [K in keyof Values]: ActionPort<Values[K]> };

/** What `setup` receives: the instance's ports, its lifetime signal, telemetry and ownership helpers. */
export interface ActionComponentContext<Inputs extends object, Outputs extends object> {
  readonly inputs: ActionPorts<Inputs>;
  readonly outputs: ActionPorts<Outputs>;
  readonly signal: AbortSignal;
  readonly telemetry: ActionTelemetry | undefined;
  /** Cleanup functions run once in reverse registration order. */
  own(cleanup: ActionCleanup): ActionCleanup;
  /** A new Action owned by the component and disposed with it. */
  action<T>(name: string): Action<T>;
  /** Acquires a resource and owns its disposal; disposes it at once when the component was destroyed meanwhile. */
  resource<T>(acquire: () => ActionAsync<T>, dispose: (resource: T) => ActionAsync<void>): Promise<T>;
}
/** A component's name, its port topology and the setup that wires inputs to outputs. */
export interface ActionComponentDefinition<Inputs extends object, Outputs extends object> {
  readonly name: string;
  readonly inputs: ActionPortDefinitions<Inputs>;
  readonly outputs: ActionPortDefinitions<Outputs>;
  /** Runs on `start()`; a returned cleanup runs at `destroy()`. */
  readonly setup?: (context: ActionComponentContext<Inputs, Outputs>) => ActionAsync<void | ActionCleanup>;
}
/** A defined component; every `create()` is an independent instance with fresh ports. */
export interface ActionComponentTemplate<Inputs extends object, Outputs extends object> {
  readonly name: string;
  create(options?: ActionLifecycleOptions): ActionComponent<Inputs, Outputs>;
}

function copyPorts<Values extends object>(definitions: ActionPortDefinitions<Values>): ActionPortDefinitions<Values> {
  if (!definitions || typeof definitions !== 'object' || Array.isArray(definitions)) throw new TypeError('Invalid port definitions');
  const ports: Record<string, ActionPort<unknown>> = Object.create(null) as Record<string, ActionPort<unknown>>;
  for (const key of Reflect.ownKeys(definitions)) {
    if (typeof key !== 'string' || !key.trim() || key.length > 200) throw new TypeError('Invalid port name');
    const value = (definitions as Record<string, ActionPort<unknown>>)[key];
    if (!value || value[portMarker] !== true) throw new TypeError('Use actionPort<T>() for component ports');
    ports[key] = value;
  }
  return Object.freeze(ports) as ActionPortDefinitions<Values>;
}

function copyDefinition<Inputs extends object, Outputs extends object>(
  definition: ActionComponentDefinition<Inputs, Outputs>,
): ActionComponentDefinition<Inputs, Outputs> {
  if (definition.setup !== undefined && typeof definition.setup !== 'function') throw new TypeError('Component setup must be a function');
  return Object.freeze({ ...definition, inputs: copyPorts(definition.inputs), outputs: copyPorts(definition.outputs) });
}

/** Typed Action ports with one owner. Wire observers before start(), destroy at the lifetime boundary. */
export class ActionComponent<Inputs extends object, Outputs extends object> extends ActionLifecycle {
  readonly inputs: ActionPorts<Inputs>;
  readonly outputs: ActionPorts<Outputs>;
  readonly #definition: ActionComponentDefinition<Inputs, Outputs>;
  readonly #context: ActionComponentContext<Inputs, Outputs>;

  constructor(definition: ActionComponentDefinition<Inputs, Outputs>, options: ActionLifecycleOptions = {}) {
    const copied = copyDefinition(definition);
    super(copied.name, options);
    this.#definition = copied;
    this.inputs = this.#ports(copied.inputs, 'input');
    this.outputs = this.#ports(copied.outputs, 'output');
    this.#context = Object.freeze({
      inputs: this.inputs, outputs: this.outputs, signal: this.signal, telemetry: this.telemetry,
      own: (cleanup: ActionCleanup) => this.own(cleanup),
      action: <T>(name: string) => this.#action<T>(name),
      resource: async <T>(acquire: () => ActionAsync<T>, dispose: (resource: T) => ActionAsync<void>) => {
        this.assertAlive();
        const value = await acquire();
        if (this.signal.aborted) { await dispose(value); this.signal.throwIfAborted(); }
        this.own(() => dispose(value));
        return value;
      },
    });
  }

  #action<T>(name: string): Action<T> {
    this.assertAlive();
    const action = new Action<T>({
      name: this.name + '.' + name,
      ...(this.telemetry ? { telemetry: this.telemetry } : {}),
      onUnhandledError: error => this.reportError(error),
    });
    this.own(() => action.dispose());
    return action;
  }

  #ports<Values extends object>(definitions: ActionPortDefinitions<Values>, direction: string): ActionPorts<Values> {
    const ports: Record<string, Action<unknown>> = Object.create(null) as Record<string, Action<unknown>>;
    for (const name of Object.keys(definitions)) ports[name] = this.#action(direction + '.' + name);
    return Object.freeze(ports) as ActionPorts<Values>;
  }

  /** Override this in class templates; setup is never called from the constructor. */
  protected configure(context: ActionComponentContext<Inputs, Outputs>): ActionAsync<void | ActionCleanup> {
    return this.#definition.setup?.(context);
  }
  protected setup(_signal: AbortSignal): ActionAsync<void | ActionCleanup> { return this.configure(this.#context); }
}

/** Defines reusable port topology once; each create() gets fresh channels and ownership. */
export function defineActionComponent<Inputs extends object, Outputs extends object>(
  definition: ActionComponentDefinition<Inputs, Outputs>,
): ActionComponentTemplate<Inputs, Outputs> {
  const copied = copyDefinition(definition);
  return Object.freeze({ name: copied.name, create: (options?: ActionLifecycleOptions) => new ActionComponent(copied, options) });
}
