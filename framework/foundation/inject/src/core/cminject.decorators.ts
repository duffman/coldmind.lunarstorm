import { isDeferredToken, type DependencySlot } from '@coldmind/inject/lazy';
import 'reflect-metadata';
import type {
  ClassDescriptor, ClassRegistration, DecoratorPlacement,
  DecoratorSpec, DecorationTarget, InjectableOptions, InjectDecorator, InjectableDecorator,
  LifetimeOptions, MemberInjection, MethodRecord, ParameterInjection,
  RegistrationContribution, StaticInjection, UniversalDecorator, ValidatorRecord, WrapRecord,
} from '@coldmind/inject/contracts';
import { ServiceLifetime } from '@coldmind/inject/contracts';
import { DelayedConstructor, type InjectionToken, type Constructor } from '@coldmind/inject/token';
import { classChain, createRecords, placementPhrase, siteOf } from '@coldmind/decorate';

/** Metadata key under which the legacy decorator mode stores a class descriptor. */
export const INJECTABLE_METADATA_KEY = Symbol('cminject:injectable');
/** Metadata key for injected parameter and member tokens. */
export const INJECT_METADATA_KEY = Symbol('cminject:inject');
/** Metadata key marking optional injections. */
export const INJECT_OPTIONAL_METADATA_KEY = Symbol('cminject:optional');
/** Metadata key marking multi-injections (`@InjectAll`). */
export const INJECT_ALL_METADATA_KEY = Symbol('cminject:all');

/** What decorators and static declarations collected for one class, before base classes are merged in. */
interface MetadataRecord {
  /** Registration data, when declared. */
  registration?: ClassRegistration;
  /** Constructor parameters to inject. */
  params: ParameterInjection[];
  /** Properties to inject. */
  members: MemberInjection[];
  /** Methods marked for discovery. */
  methods: MethodRecord[];
  /** Method wrappers. */
  wraps: WrapRecord[];
  /** Registration checks. */
  validators: ValidatorRecord[];
  /** Discovery tags. */
  tags: string[];
}
/** What decorators wrote, by class, in both decorator dialects. */
const store = createRecords<MetadataRecord>(() => ({ params: [], members: [], methods: [], wraps: [], validators: [], tags: [] }));
/** `Reflect` with the `reflect-metadata` methods, typed. */
const metadataReflect: {
  /** Writes metadata on a target. Matches `reflect-metadata`. */
  defineMetadata(key: unknown, value: unknown, target: object): void;
  /** Reads metadata from a target and its prototype chain. */
  getMetadata(key: unknown, target: object): unknown;
  /** Reads metadata stored directly on a target. */
  getOwnMetadata(key: unknown, target: object): unknown;
} = Reflect;
/** Reads a `$inject` or `$members` entry: a bare token, or a token with `optional` and `all`. */
const parseStatic = (value: StaticInjection | undefined): { token?: DependencySlot; optional: boolean; multi: boolean } => {
  if (value === undefined) return { token: undefined, optional: false, multi: false };
  if (isDeferredToken(value)) return { token: value, optional: false, multi: false };
  if (value && typeof value === 'object' && !(value instanceof DelayedConstructor) && 'token' in value) {
    return { token: value.token, optional: !!value.optional, multi: !!value.all };
  }
  return { token: value, optional: false, multi: false };
};

/** Writes authored member slots with the same strict failure semantics as direct assignment. */
function setMember(instance: object, name: string | symbol, value: unknown): void {
  if (!Reflect.set(instance, name, value)) throw new TypeError(`Cannot inject ${String(name)}`);
}

/** A class with the optional static declarations cminject reads. */
type AuthoredClass = Function & {
  $inject?: StaticInjection[];
  $members?: Record<string | symbol, StaticInjection>;
  $injectable?: InjectableOptions;
};

/** Types TypeScript reflects for a parameter typed as an interface, union, primitive or `any`. None is a class to resolve by. */
const notTokens = new Set<unknown>([Object, Function, Array, String, Number, Boolean, Symbol, BigInt, Promise]);

/** Returns the merged descriptor, walking base classes before derived classes. */
export function describe<T>(ctor: Constructor<T>): ClassDescriptor {
  const result: ClassDescriptor = { params: [], members: [], methods: [], wraps: [], validators: [], tags: [] };
  const members = new Map<string | symbol, MemberInjection>();
  const params = new Map<number, ParameterInjection>();
  const classes = classChain(ctor);
  for (const type of classes) {
    const own = store.of(type);
    const authored: AuthoredClass = type;
    if (type === ctor) {
      if (own?.registration && Object.hasOwn(type, '$injectable')) {
        throw new TypeError(`${type.name} is registered by ${own.registration.source} and by static $injectable; declare it once`);
      }
      if (own?.registration) result.registration = own.registration;
      else if (Object.hasOwn(type, '$injectable') && authored.$injectable) {
        const { tags, ...reg } = authored.$injectable;
        result.registration = { ...reg, source: 'static $injectable' };
      }
    }
    for (const param of own?.params ?? []) params.set(param.index, { ...param });
    for (const member of own?.members ?? []) members.set(member.name, member);
    if (Object.hasOwn(type, '$members') && authored.$members) {
      for (const key of Reflect.ownKeys(authored.$members)) {
        if (own?.members.some(member => member.name === key)) throw new TypeError(`${type.name}.${String(key)} is injected by a decorator and by static $members; declare it once`);
        const slot = parseStatic(authored.$members[key]);
        if (slot.token === undefined) continue;
        members.set(key, { name: key, token: slot.token, optional: slot.optional, multi: slot.multi,
          source: 'static $members', set: (instance, value) => { setMember(instance, key, value); } });
      }
    }
    result.methods.push(...(own?.methods ?? []));
    result.wraps.push(...(own?.wraps ?? []));
    result.validators.push(...(own?.validators ?? []));
    for (const tag of [...(own?.tags ?? []), ...(type === ctor && Object.hasOwn(type, '$injectable') ? authored.$injectable?.tags ?? [] : [])]) {
      if (!result.tags.includes(tag)) result.tags.push(tag);
    }
  }
  const regDeps = result.registration?.deps;
  const authored: AuthoredClass = ctor;
  const staticDeps = authored.$inject ?? [];
  // TypeScript's design:paramtypes metadata is an array when emitted; absence remains undefined.
  const reflected = metadataReflect.getMetadata('design:paramtypes', ctor) as unknown[] | undefined;
  const highest = params.size ? Math.max(...params.keys()) + 1 : 0;
  const count = Math.max(reflected?.length ?? 0, highest, regDeps?.length ?? 0, regDeps ? 0 : staticDeps.length, ctor.length);
  for (let index = 0; index < count; index++) {
    const decorated = params.get(index);
    const declared = parseStatic(regDeps ? regDeps[index] : staticDeps[index]);
    const reflectedToken = reflected?.[index];
    // Reflected nonprimitive functions are class tokens emitted by TypeScript; explicit metadata takes precedence.
    const token = decorated?.token ?? declared.token ?? (typeof reflectedToken === 'function' && !notTokens.has(reflectedToken) ? reflectedToken as Constructor : undefined);
    result.params.push({ index, token, optional: (decorated?.optional ?? false) || declared.optional, multi: (decorated?.multi ?? false) || declared.multi });
  }
  result.members = [...members.values()];
  return result;
}

/** Author a single decorator implementation across legacy and stage-3 syntax. */
export function defineDecorator<TArgs extends unknown[]>(spec: DecoratorSpec<TArgs>): (...args: TArgs) => UniversalDecorator {
  return (...args: TArgs): UniversalDecorator => {
    const decorator = (value: unknown, arg?: unknown, third?: unknown): void => {
      const site = siteOf(value, arg, third);
      const { placement, name, index, ctor, static: isStatic } = site;
      if (!spec.on.includes(placement)) {
        const member = name === undefined ? '' : ` (${String(name)})`;
        const hint = site.dialect === 'standard' && spec.on.includes('parameter') ? '; under standard decorators inject constructor parameters with `static $inject`' : '';
        throw new TypeError(`${spec.name} goes on ${spec.on.map(p => placementPhrase[p]).join(' or ')}, not ${placementPhrase[placement]}${member}${hint}`);
      }
      if (site.dialect === 'standard' && placement !== 'class' && !site.metadata) {
        throw new TypeError('This runtime passes standard decorators no metadata object; cminject needs one to see member decorators');
      }
      const record = store.at(site);
      const requirePlacement = (op: string, allowed: DecoratorPlacement[]) => {
        if (!allowed.includes(placement)) throw new TypeError(`${spec.name}: ${op}() is not available on ${placementPhrase[placement]}`);
      };
      const target: DecorationTarget = {
        dialect: site.dialect, placement, name, index, static: isStatic,
        register(contribution: RegistrationContribution) {
          requirePlacement('register', ['class']);
          if (record.registration) throw new TypeError(`${spec.name} conflicts with ${record.registration.source}: a class takes one registration`);
          record.registration = { ...contribution, source: spec.name };
          if (ctor) metadataReflect.defineMetadata(INJECTABLE_METADATA_KEY, { ...contribution, token: contribution.token ?? ctor }, ctor);
        },
        tag(tag: string) { requirePlacement('tag', ['class']); if (!record.tags.includes(tag)) record.tags.push(tag); },
        inject(injection) {
          requirePlacement('inject', ['field', 'accessor', 'setter', 'parameter']);
          if (isStatic) throw new TypeError(`${spec.name} cannot inject a static member`);
          if (placement === 'parameter') {
            if (name !== undefined) throw new TypeError(`${spec.name} cannot inject a method parameter; constructor parameters only`);
            if (injection.derive) throw new TypeError(`${spec.name} cannot derive a constructor parameter`);
            const entry: ParameterInjection = { index: index!, token: injection.token, optional: !!injection.optional, multi: !!injection.all };
            record.params = record.params.filter(p => p.index !== index); record.params.push(entry);
            // INJECT_METADATA_KEY is written only as this array of parameter tokens.
            const types = metadataReflect.getOwnMetadata(INJECT_METADATA_KEY, ctor!) as Array<DependencySlot | undefined> | undefined;
            const tokens = types ? [...types] : []; tokens[index!] = injection.token;
            metadataReflect.defineMetadata(INJECT_METADATA_KEY, tokens, ctor!);
            if (injection.optional) {
              // Optional-parameter metadata is written only as a Set<number>.
              const prev = metadataReflect.getOwnMetadata(INJECT_OPTIONAL_METADATA_KEY, ctor!) as Set<number> | undefined;
              metadataReflect.defineMetadata(INJECT_OPTIONAL_METADATA_KEY, new Set([...(prev ?? []), index!]), ctor!);
            }
            if (injection.all) {
              // Multi-parameter metadata is written only as a Set<number>.
              const prev = metadataReflect.getOwnMetadata(INJECT_ALL_METADATA_KEY, ctor!) as Set<number> | undefined;
              metadataReflect.defineMetadata(INJECT_ALL_METADATA_KEY, new Set([...(prev ?? []), index!]), ctor!);
            }
            return;
          }
          if (name === undefined) throw new TypeError(`${spec.name} requires a member name`);
          const collision = record.members.find(m => m.name === name);
          if (collision) throw new TypeError(`${spec.name} and ${collision.source} both inject ${String(name)}`);
          const set: MemberInjection['set'] = site.access?.set
            ? (instance, resolved) => site.access!.set!(instance, resolved)
            : (instance, resolved) => { setMember(instance, name, resolved); };
          record.members.push({ name, token: injection.token, optional: !!injection.optional, multi: !!injection.all, derive: injection.derive, source: spec.name, set });
        },
        method(options) {
          requirePlacement('method', ['method']);
          if (isStatic) throw new TypeError(`${spec.name} cannot record a static method`);
          // The normalized method site supplies a callable member, including private TC39 access.
          record.methods.push({ name: name!, decorator: spec.name, options,
            get: site.access?.get ? (instance) => (site.access!.get!(instance) as (...a: unknown[]) => unknown).bind(instance)
              : (instance) => ((instance as Record<string | symbol, (...a: unknown[]) => unknown>)[name!]!).bind(instance) });
        },
        wrap(wrap) {
          requirePlacement('wrap', ['method']);
          if (isStatic) throw new TypeError(`${spec.name} cannot wrap a static method`);
          if (site.private) throw new TypeError(`${spec.name} cannot wrap a #private method`);
          record.wraps.push({ name: name!, decorator: spec.name, wrap });
        },
        validate(check) { record.validators.push({ decorator: spec.name, check }); },
      };
      spec.apply(target, ...args);
    };
    return decorator;
  };
}

/** Marks a class as injectable. Accepts `lifetime`, `token`, `deps` and `tags`. */
export const Injectable: (options?: InjectableOptions) => InjectableDecorator = defineDecorator<[InjectableOptions?]>({
  name: '@Injectable', on: ['class'],
  apply(target, options = {}) { const { tags = [], ...registration } = options; target.register(registration); for (const tag of tags) target.tag(tag); }
});

/** Builds a class decorator that registers with a fixed lifetime, taking a token or options. */
function lifetimeDecorator(name: string, lifetime: ServiceLifetime): (tokenOrOptions?: InjectionToken | LifetimeOptions) => InjectableDecorator {
  return defineDecorator<[tokenOrOptions?: InjectionToken | LifetimeOptions]>({
    name, on: ['class'],
    apply(target, tokenOrOptions) {
      const options = tokenOrOptions != null && typeof tokenOrOptions === 'object' && !(tokenOrOptions instanceof DelayedConstructor)
        ? tokenOrOptions : { token: tokenOrOptions };
      const { tags = [], ...registration } = options;
      target.register({ ...registration, lifetime });
      for (const tag of tags) target.tag(tag);
    }
  });
}
/** Sets the class lifetime to singleton. */
export const Singleton = lifetimeDecorator('@Singleton', ServiceLifetime.Singleton);
/** Sets the class lifetime to transient. */
export const Transient = lifetimeDecorator('@Transient', ServiceLifetime.Transient);
/** Sets the class lifetime to scoped. */
export const Scoped = lifetimeDecorator('@Scoped', ServiceLifetime.Scoped);
/** Sets the class lifetime to tenant-scoped. */
export const Tenant = lifetimeDecorator('@Tenant', ServiceLifetime.Tenant);
/** Sets the class lifetime to request-scoped. */
export const Request = lifetimeDecorator('@Request', ServiceLifetime.Request);

/** Injects the given token into a parameter, field or accessor. */
export const Inject: (token: DependencySlot) => InjectDecorator = defineDecorator<[DependencySlot]>({
  name: '@Inject', on: ['field', 'accessor', 'setter', 'parameter'],
  apply(target, token) { target.inject({ token }); }
});
/** Like `@Inject`, but resolves to `undefined` when the token is unregistered. */
export const InjectOptional: (token: DependencySlot) => InjectDecorator = defineDecorator<[DependencySlot]>({
  name: '@InjectOptional', on: ['field', 'accessor', 'setter', 'parameter'],
  apply(target, token) { target.inject({ token, optional: true }); }
});
/** Injects every multi-registration for the token. */
export const InjectAll: (token: DependencySlot) => InjectDecorator = defineDecorator<[DependencySlot]>({
  name: '@InjectAll', on: ['field', 'accessor', 'setter', 'parameter'],
  apply(target, token) { target.inject({ token, all: true }); }
});
