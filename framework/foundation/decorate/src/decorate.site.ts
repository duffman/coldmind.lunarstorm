/** Where a decorator is applied. */
export type Placement = 'class' | 'field' | 'accessor' | 'setter' | 'getter' | 'method' | 'parameter';
/** Decorator dialect: legacy (`experimentalDecorators`) or standard (TC39). */
export type Dialect = 'legacy' | 'standard';

/** The context object a standard decorator receives. */
export interface StandardContext {
  /** What the decorator is applied to. */
  kind: Exclude<Placement, 'parameter'>;
  /** Member name. Absent for a class, possibly absent for an anonymous class. */
  name?: string | symbol;
  /** Whether the member is static. */
  static?: boolean;
  /** Whether the member is a `#private` one. */
  private?: boolean;
  /** Metadata object shared by every decorator of one class. Absent on runtimes that do not provide it. */
  metadata?: object;
  /** Reads and writes the member on an instance, `#private` members included. */
  access?: { get?(obj: object): unknown; set?(obj: object, value: unknown): void };
}

/** One decorator application, seen the same way in both dialects. */
export interface DecoratorSite {
  /** Which dialect applied the decorator. */
  dialect: Dialect;
  /** Where the decorator was applied. */
  placement: Placement;
  /** Member name. `undefined` for a class or a constructor parameter. */
  name: string | symbol | undefined;
  /** Parameter index for a parameter decorator. `undefined` otherwise. */
  index: number | undefined;
  /** Whether the member is static. */
  static: boolean;
  /** Whether the member is `#private`. Only standard decorators can tell. */
  private: boolean;
  /** The class decorated, or the class a legacy member belongs to. `undefined` for a standard member: the class does not exist yet. */
  ctor: Function | undefined;
  /** The standard metadata object, when the runtime provides one. */
  metadata: object | undefined;
  /** The standard member accessors, when the runtime provides them. */
  access: StandardContext['access'];
}

/** Placement names for error messages: "a class", "a getter-only accessor". */
export const placementPhrase: Record<Placement, string> = {
  class: 'a class', field: 'a field', accessor: 'an accessor', setter: 'a setter', getter: 'a getter-only accessor', method: 'a method', parameter: 'a parameter'
};

/** Whether `arg` is the context object of a standard decorator. */
function isStandardContext(arg: unknown): arg is StandardContext {
  return !!arg && typeof arg === 'object' && typeof (arg as { kind?: unknown }).kind === 'string';
}

/** Works out where a legacy decorator was applied from the arguments it received. */
function legacyPlacement(target: unknown, key?: string | symbol, descriptor?: unknown): Placement {
  if (typeof descriptor === 'number') return 'parameter';
  if (typeof target === 'function' && key === undefined) return 'class';
  if (!descriptor) return 'field';
  const item = descriptor as PropertyDescriptor;
  if (typeof item.value === 'function') return 'method';
  if (item.get && item.set) return 'accessor';
  if (item.set) return 'setter';
  return 'getter';
}

/**
 * Reads the arguments a decorator function received, whichever dialect called it.
 * Standard decorators get `(value, context)`; legacy ones get `(target, key, descriptorOrIndex)`.
 */
export function siteOf(value: unknown, arg?: unknown, third?: unknown): DecoratorSite {
  if (isStandardContext(arg)) {
    return {
      dialect: 'standard', placement: arg.kind, name: arg.name, index: undefined, static: !!arg.static, private: !!arg.private,
      ctor: arg.kind === 'class' ? value as Function : undefined, metadata: arg.metadata, access: arg.access,
    };
  }
  const name = arg as string | symbol | undefined;
  return {
    dialect: 'legacy', placement: legacyPlacement(value, name, third), name, index: typeof third === 'number' ? third : undefined,
    static: typeof value === 'function' && name !== undefined, private: false,
    ctor: typeof value === 'function' ? value : (value as object).constructor, metadata: undefined, access: undefined,
  };
}
