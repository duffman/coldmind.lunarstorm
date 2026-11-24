import { metadataKey } from '@coldmind/decorate/internal/metadata';
import type { DecoratorSite } from '@coldmind/decorate/internal/site';

/** The records one library keeps for decorated classes, in both dialects. */
export interface Records<R> {
  /**
   * The record a decorator at `site` writes to, created on first use. A standard decorator is keyed on its class's
   * metadata object, so a member never attaches to another class and no class decorator is needed. A legacy one is keyed on the constructor.
   * A class site also links the class to its record, for runtimes that pass standard decorators no metadata.
   */
  at(site: DecoratorSite): R;
  /** The record collected for this class itself, base classes not merged in; `undefined` when nothing was decorated. */
  of(ctor: Function): R | undefined;
}

/** Creates a record store. Each store has its own slot, so libraries sharing a metadata object do not see each other's records. */
export function createRecords<R extends object>(make: () => R): Records<R> {
  /** Property on a standard metadata object that holds its own record, so a subclass does not share its parent's. */
  const slot = Symbol('decorate:record');
  /** Records written by legacy decorators, by class. */
  const legacy = new WeakMap<Function, R>();
  /** Records written by standard decorators, by metadata object. */
  const standard = new WeakMap<object, R>();
  /** Metadata object of each class decorated with a standard class decorator. */
  const classMetadata = new WeakMap<Function, object>();

  const legacyRecord = (ctor: Function): R => {
    let record = legacy.get(ctor);
    if (!record) { record = make(); legacy.set(ctor, record); }
    return record;
  };
  const standardRecord = (metadata: object): R => {
    let record = standard.get(metadata);
    if (!record) {
      record = Object.hasOwn(metadata, slot) ? (metadata as Record<symbol, R>)[slot]! : make();
      Object.defineProperty(metadata, slot, { value: record, configurable: true });
      standard.set(metadata, record);
    }
    return record;
  };

  return {
    at(site) {
      if (site.dialect === 'legacy') return legacyRecord(site.ctor!);
      // A class decorator on a runtime without metadata gets one object, used for both its record and the class lookup.
      const metadata = site.metadata ?? (site.placement === 'class' ? classMetadata.get(site.ctor!) ?? {} : {});
      const record = standardRecord(metadata);
      if (site.placement === 'class') classMetadata.set(site.ctor!, metadata);
      return record;
    },
    of(ctor) {
      const metadata = classMetadata.get(ctor) ?? (Object.hasOwn(ctor, metadataKey)
        ? (ctor as unknown as Record<symbol, object | undefined>)[metadataKey] : undefined);
      const own = metadata && (standard.get(metadata) ?? (Object.hasOwn(metadata, slot) ? (metadata as Record<symbol, R>)[slot] : undefined));
      return own ?? legacy.get(ctor);
    },
  };
}

/** A class and its base classes, base first. */
export function classChain(ctor: Function): Function[] {
  const out: Function[] = [];
  for (let c: unknown = ctor; typeof c === 'function' && c !== Function.prototype; c = Object.getPrototypeOf(c)) out.unshift(c);
  return out;
}
