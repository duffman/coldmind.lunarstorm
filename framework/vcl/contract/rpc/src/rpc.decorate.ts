// ---- defineDecorator: one implementation, both decorator dialects ----
//
// TypeScript has two decorator dialects with different call shapes:
//   standard (TC39, what Bun runs without a compiler):  class (value, context)   method (value, context)
//   legacy (`experimentalDecorators`, a compiler emit):  class (ctor)             method (target, key, descriptor)
// `defineDecorator` hides the difference. The implementation receives a uniform `DecoratorSite`.

/** The class a decorator sits on, or the class a decorated method belongs to. */
export type ClassSite = {
  readonly kind: "class";
  /** The class name; `""` for an anonymous class. */
  readonly name: string;
  readonly target: Function;
};

/** A decorated method (instance or static). */
export type MethodSite = {
  readonly kind: "method";
  readonly name: string | symbol;
  readonly isStatic: boolean;
  /** The original function. */
  readonly method: Function;
  /**
   * Runs `callback` with the owning class once it exists. Legacy decorators know the class at once, so the
   * callback runs immediately. Standard member decorators run before the class does: the callback runs when
   * the class's own class decorator (any decorator made by {@link defineDecorator} with a class site) runs.
   * A class that has member decorators but no class decorator never calls it under the standard dialect.
   */
  onClass(callback: (ctor: Function) => void): void;
};

/**
 * A decorated field (instance or static). `accessor` is true for a standard `accessor x` member, the
 * only kind whose reads and writes can be wrapped under the standard dialect; legacy decorators wrap
 * any property by installing a getter/setter on the prototype.
 */
export type FieldSite = {
  readonly kind: "field";
  readonly name: string | symbol;
  readonly isStatic: boolean;
  readonly accessor: boolean;
  /** As {@link MethodSite.onClass}. */
  onClass(callback: (ctor: Function) => void): void;
};

export type DecoratorSite = ClassSite | MethodSite | FieldSite;

/**
 * What an implementation returns for a field: how to wrap the member. Every part is optional.
 * `get`/`set` need an `accessor` member under the standard dialect (a plain field throws), and any property
 * under the legacy one. `init` replaces the initial value and works on both kinds of field.
 */
export type FieldWrap = {
  /** Wraps a read. `read()` returns the stored value. */
  get?(this: object, read: () => unknown): unknown;
  /** Wraps a write. `write(value)` stores it; `read()` returns what is stored now. */
  set?(this: object, value: unknown, write: (value: unknown) => void, read: () => unknown): void;
  /**
   * Maps the initial value (`undefined` when the field has no initializer) to the stored one. Under the legacy
   * dialect a field cannot tell its initializer from a later assignment, so its first assignment is taken as the
   * initial value and does not call `set`: give a wrapped field an initializer (`= value`), as every `@prop` has.
   */
  init?(this: object, initial: unknown): unknown;
};

/** What an implementation may return: nothing, a replacement function (for a class: a subclass or the class itself), or for a field its {@link FieldWrap}. */
export type DecoratorResult = void | Function | FieldWrap;

/** The decorator `defineDecorator` returns. Usable on classes, methods, fields and `accessor`s in either dialect. */
export interface Decorator {
  // standard
  <T extends Function>(value: T, context: ClassDecoratorContext): T | void;
  <T extends Function>(value: T, context: ClassMethodDecoratorContext<any, any>): T | void;
  <This, V>(value: undefined, context: ClassFieldDecoratorContext<This, V>): void | ((this: This, initial: V) => V);
  <This, V>(value: ClassAccessorDecoratorTarget<This, V>, context: ClassAccessorDecoratorContext<This, V>): ClassAccessorDecoratorResult<This, V> | void;
  // legacy
  <T extends Function>(target: T): T | void;
  (target: object, key: string | symbol, descriptor?: PropertyDescriptor): PropertyDescriptor | void;
}

type Pending = { metadata: unknown; name: string; callback: (ctor: Function) => void };
const pending: Pending[] = [];

/**
 * The members, named `Class.member` when known, whose class never ran a class decorator of this mechanism, so
 * their callbacks have not run. Standard dialect only: legacy decorators know their class at once. Non-empty
 * after startup means a decorated class lacks its class decorator (`@Controller`, `@ViewForm`, ...).
 */
export function strayMembers(): string[] {
  return pending.map(p => p.name);
}

const isStandardContext = (c: unknown): c is { kind: string; name: string | symbol | undefined; static?: boolean; metadata?: object } =>
  typeof c === "object" && c !== null && typeof (c as { kind?: unknown }).kind === "string";

/** Hands the class to the member callbacks queued for it. Matched by `context.metadata`, which Bun shares per class. */
function flush(ctor: Function, metadata: unknown): void {
  const matches = (p: Pending) => p.metadata === metadata || p.metadata === undefined || metadata === undefined;
  const mine = pending.filter(matches);
  for (const p of mine) pending.splice(pending.indexOf(p), 1);
  for (const p of mine) p.callback(ctor);
}

/**
 * A legacy property decorator: installs a getter/setter on the owner (prototype, or class for a static) that
 * keeps each instance's value in a WeakMap. The first write is the field's initialization, as an `init` in the
 * standard dialect: it goes through `init`, not `set`. A read before any write initializes with `undefined`.
 * Needs `useDefineForClassFields: false`, or the instance's own property shadows this one.
 */
function legacyField<A extends unknown[]>(
  owner: object,
  key: string | symbol,
  isStatic: boolean,
  ctor: Function,
  args: A,
  implementation: (site: DecoratorSite, ...args: A) => DecoratorResult,
): undefined {
  const site: FieldSite = { kind: "field", name: key, isStatic, accessor: false, onClass: callback => callback(ctor) };
  const wrap = implementation(site, ...args) as FieldWrap | void;
  if (!wrap || (!wrap.get && !wrap.set && !wrap.init)) return undefined;
  const stored = new WeakMap<object, unknown>();
  const read = (self: object): unknown => {
    if (!stored.has(self)) stored.set(self, wrap.init ? wrap.init.call(self, undefined) : undefined);
    return stored.get(self);
  };
  Object.defineProperty(owner, key, {
    configurable: true,
    enumerable: true,
    get(this: object): unknown {
      return wrap.get ? wrap.get.call(this, () => read(this)) : read(this);
    },
    set(this: object, value: unknown): void {
      if (!stored.has(this)) {
        stored.set(this, wrap.init ? wrap.init.call(this, value) : value);
        return;
      }
      const write = (next: unknown): void => void stored.set(this, next);
      if (wrap.set) wrap.set.call(this, value, write, () => read(this));
      else write(value);
    },
  });
  return undefined;
}

/**
 * Builds a decorator factory that works under both dialects, without a compiler (Bun's native standard
 * decorators) and with `experimentalDecorators` emit. Handles class, method and field decorators (a field is
 * a property, or under the standard dialect also an `accessor`); the implementation sees one `DecoratorSite`
 * either way and may return a replacement function, or for a field a {@link FieldWrap}.
 *
 * @param name used in the error thrown when the decorator is applied to something unsupported.
 * @param implementation called once per decorated class, method or field, with the site and the factory's arguments.
 * @returns the factory: `factory(...args)` is the decorator to put in front of a class, method or field.
 * @throws {TypeError} When applied to a getter, setter or parameter, when a wrapped field is a plain (non-`accessor`)
 *   standard field, or when called in an unrecognised shape.
 * @example
 * const tag = defineDecorator("tag", (site, label: string) => {
 *   if (site.kind === "method") site.onClass(ctor => labels.set(ctor, [...(labels.get(ctor) ?? []), label]));
 * });
 * class Service { @tag("hot") run() {} }
 */
export function defineDecorator<A extends unknown[]>(
  name: string,
  implementation: (site: DecoratorSite, ...args: A) => DecoratorResult,
): (...args: A) => Decorator {
  return (...args: A): Decorator =>
    ((...call: unknown[]): unknown => {
      const [first, second, third] = call;

      // standard dialect: (value, context)
      if (call.length === 2 && isStandardContext(second)) {
        if (second.kind === "class") {
          const ctor = first as Function;
          flush(ctor, second.metadata);
          return implementation({ kind: "class", name: ctor.name, target: ctor }, ...args) ?? undefined;
        }
        if (second.kind === "method") {
          const method = first as Function;
          const site: MethodSite = {
            kind: "method",
            name: second.name as string | symbol,
            isStatic: second.static === true,
            method,
            onClass: callback => void pending.push({ metadata: second.metadata, name: String(second.name), callback }),
          };
          return implementation(site, ...args) ?? undefined;
        }
        if (second.kind === "field" || second.kind === "accessor") {
          const isAccessor = second.kind === "accessor";
          const site: FieldSite = {
            kind: "field",
            name: second.name as string | symbol,
            isStatic: second.static === true,
            accessor: isAccessor,
            onClass: callback => void pending.push({ metadata: second.metadata, name: String(second.name), callback }),
          };
          const wrap = implementation(site, ...args) as FieldWrap | void;
          if (!wrap) return undefined;
          const init = wrap.init;
          if (!isAccessor) {
            if (wrap.get || wrap.set) throw new TypeError(`@${name} ${String(second.name)}: only an accessor member can wrap reads and writes; declare it \`accessor ${String(second.name)}\``);
            return init ? function (this: object, initial: unknown) { return init.call(this, initial); } : undefined;
          }
          const target = first as ClassAccessorDecoratorTarget<object, unknown>;
          const result: ClassAccessorDecoratorResult<object, unknown> = {
            get(this: object) {
              const read = () => target.get.call(this);
              return wrap.get ? wrap.get.call(this, read) : read();
            },
            set(this: object, value: unknown) {
              const write = (next: unknown) => target.set.call(this, next);
              if (wrap.set) wrap.set.call(this, value, write, () => target.get.call(this));
              else write(value);
            },
          };
          if (init) result.init = function (this: object, initial: unknown) { return init.call(this, initial); };
          return result;
        }
        throw new TypeError(`@${name} can decorate classes, methods and fields, not ${second.kind}s`);
      }

      // legacy dialect, class: (ctor)
      if (call.length === 1 && typeof first === "function") {
        const ctor = first;
        return implementation({ kind: "class", name: ctor.name, target: ctor }, ...args) ?? undefined;
      }

      // legacy dialect, method: (target, key, descriptor)
      if (call.length >= 2 && (typeof first === "object" || typeof first === "function") && first !== null && (typeof second === "string" || typeof second === "symbol")) {
        const descriptor = third as PropertyDescriptor | undefined;
        const isStatic = typeof first === "function";
        const ctor = (isStatic ? first : (first as { constructor: Function }).constructor) as Function;
        if (!descriptor) return legacyField(first, second, isStatic, ctor, args, implementation);
        if (typeof descriptor.value !== "function") throw new TypeError(`@${name} can decorate classes, methods and fields, not getters or setters`);
        const site: MethodSite = {
          kind: "method",
          name: second,
          isStatic,
          method: descriptor.value as Function,
          onClass: callback => callback(ctor),
        };
        const replaced = implementation(site, ...args);
        return typeof replaced === "function" ? { ...descriptor, value: replaced } : undefined;
      }

      throw new TypeError(`@${name} was applied in an unsupported way`);
    }) as Decorator;
}
