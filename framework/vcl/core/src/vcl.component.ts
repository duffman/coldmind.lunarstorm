/**
 * Component: the non-visual base. State and behaviour live on the instance; React only renders it.
 * An owner destroys what it owns (as in Delphi). `changed()` bumps a version that `ControlHost`
 * reads through `useSyncExternalStore`, so only the changed component re-renders.
 */
import type { Container } from "@coldmind/inject";
import { kernel } from "@coldmind/vcl.core/src/vcl.kernel.ts";

type Reserved = "owner" | "components" | "controls" | "destroyed" | "id" | "subscribe" | "version" | "jsxProps";

/** What `assign` may set: data properties and `on*` event slots, not methods. */
export type Assignable<T> = {
  [K in keyof T as K extends Reserved ? never : K extends `on${string}` ? K : T[K] extends (...args: never[]) => unknown ? never : K]?: T[K];
};

/** A component as plain data: its class, name, `@prop` values and owned components. */
export type ComponentDefinition = {
  type: string;
  name: string;
  props: Record<string, unknown>;
  components: ComponentDefinition[];
};

let lastId = 0;
const propNames = new WeakMap<Function, Set<string>>();

export abstract class Component {
  /** Stable identity: React key and trace id. */
  readonly id = ++lastId;
  readonly owner: Component | undefined;
  name: string;

  readonly #owned = new Set<Component>();
  readonly #subscribers = new Set<() => void>();
  readonly #teardowns: (() => void)[] = [];
  #version = 0;
  #updating = 0;
  #pending = false;
  #destroyed = false;

  constructor(owner?: Component, name = "") {
    this.owner = owner;
    this.name = name;
    if (owner) owner.#owned.add(this);
  }

  /** Owned components, in creation order. */
  get components(): readonly Component[] {
    return [...this.#owned];
  }

  get destroyed(): boolean {
    return this.#destroyed;
  }

  findComponent(name: string): Component | undefined {
    for (const c of this.#owned) if (c.name === name) return c;
    return undefined;
  }

  /** Runs once the owner tree is built (`Application.createForm`, `load`): owned components first. */
  loaded(): void {}

  /** Notifies subscribers. Between `beginUpdate` and the outermost `endUpdate`, notifies once at the end. */
  changed(): void {
    if (this.#destroyed) return;
    if (this.#updating > 0) {
      this.#pending = true;
      return;
    }
    this.#version++;
    for (const listener of [...this.#subscribers]) listener();
  }

  beginUpdate(): void {
    this.#updating++;
  }

  /** @throws {Error} Without a matching `beginUpdate`. */
  endUpdate(): void {
    if (this.#updating === 0) throw new Error(`${this.describeName()}: endUpdate without beginUpdate`);
    if (--this.#updating === 0 && this.#pending) {
      this.#pending = false;
      this.changed();
    }
  }

  /** Sets several properties with one notification. */
  assign(values: Assignable<this>): this {
    this.beginUpdate();
    try {
      for (const [key, value] of Object.entries(values)) Reflect.set(this, key, value);
    } finally {
      this.endUpdate();
    }
    return this;
  }

  /** Registers a cleanup for `destroy`, run in reverse order. Returns `cleanup`. */
  own(cleanup: () => void): () => void {
    this.#teardowns.push(cleanup);
    return cleanup;
  }

  /** For `useSyncExternalStore`; stable identity. */
  readonly subscribe = (listener: () => void): (() => void) => {
    this.#subscribers.add(listener);
    return () => {
      this.#subscribers.delete(listener);
    };
  };

  /** For `useSyncExternalStore`; stable identity. */
  readonly version = (): number => this.#version;

  /** Routes up the owner chain; the `Application` handles it, the `Kernel` when there is none. */
  handleException(error: unknown, sender: Component = this): void {
    if (this.owner) this.owner.handleException(error, sender);
    else kernel.handleException(error, sender);
  }

  /** Destroys owned components (last created first), runs `own` cleanups, leaves the owner. */
  destroy(): void {
    if (this.#destroyed) return;
    // Set first: a destroying component no longer notifies, so its subtree tears down without re-renders.
    this.#destroyed = true;
    for (const c of [...this.#owned].reverse()) c.destroy();
    for (const cleanup of this.#teardowns.splice(0).reverse()) {
      try {
        cleanup();
      } catch (error) {
        this.handleException(error);
      }
    }
    if (this.owner) this.owner.#owned.delete(this);
    this.#subscribers.clear();
  }

  /** Calls an event slot. A throw or a rejected promise goes to `handleException`, as in Delphi. */
  protected invoke<A extends unknown[]>(handler: ((...args: A) => unknown) | undefined, ...args: A): void {
    if (!handler) return;
    try {
      const result = handler(...args);
      if (result instanceof Promise) result.catch((error: unknown) => this.handleException(error));
    } catch (error) {
      this.handleException(error);
    }
  }

  protected describeName(): string {
    return this.name || this.constructor.name;
  }
}

/**
 * A reactive property: setting a different value calls `changed()`. Recorded for `describe`.
 * @example
 * class Button extends Control { @prop accessor caption = ""; }
 */
export function prop<This extends Component, V>(
  target: ClassAccessorDecoratorTarget<This, V>,
  context: ClassAccessorDecoratorContext<This, V>,
): ClassAccessorDecoratorResult<This, V> {
  const name = String(context.name);
  context.addInitializer(function (this: This) {
    let names = propNames.get(this.constructor);
    if (!names) propNames.set(this.constructor, (names = new Set()));
    names.add(name);
  });
  return {
    get(this: This): V {
      return target.get.call(this);
    },
    set(this: This, value: V): void {
      if (Object.is(target.get.call(this), value)) return;
      target.set.call(this, value);
      this.changed();
    },
  };
}

/**
 * Injects every component in the tree under `component` (its `@Inject` fields) from `container`, then calls `loaded()`
 * on the tree, owned components first. Every `loaded()` therefore sees the whole tree injected.
 */
export function load(component: Component, container: Container = kernel.container): void {
  inject(component, container);
  loadTree(component);
}

function inject(component: Component, container: Container): void {
  container.injectMembers(component);
  for (const c of component.components) inject(c, container);
}

function loadTree(component: Component): void {
  for (const c of component.components) loadTree(c);
  component.loaded();
}

/** The `@prop` values of a component. */
export function propsOf(component: Component): Record<string, unknown> {
  const props: Record<string, unknown> = {};
  for (const name of propNames.get(component.constructor) ?? []) props[name] = Reflect.get(component, name);
  return props;
}

/** The owner tree as plain data: what a designer streams. Event slots are not included. */
export function describe(component: Component): ComponentDefinition {
  return {
    type: component.constructor.name,
    name: component.name,
    props: propsOf(component),
    components: component.components.map(describe),
  };
}
