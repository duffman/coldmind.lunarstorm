import { describe, expect, test } from 'vitest';
import { classChain, createRecords, metadataKey, siteOf, type StandardContext } from '@coldmind/decorate';

interface Notes { fields: string[]; methods: string[]; tag?: string }
const make = (): Notes => ({ fields: [], methods: [] });

function recorder(store = createRecords<Notes>(make)) {
  const field = (value: unknown, context: StandardContext) => { store.at(siteOf(value, context)).fields.push(String(context.name)); };
  const method = (value: unknown, context: StandardContext) => { store.at(siteOf(value, context)).methods.push(String(context.name)); };
  const tagged = (tag: string) => (value: unknown, context: StandardContext) => { store.at(siteOf(value, context)).tag = tag; };
  return { store, field, method, tagged };
}

describe('Symbol.metadata', () => {
  test('is defined, and is the key standard classes carry their metadata under', () => {
    expect(typeof metadataKey).toBe('symbol');
    expect((Symbol as unknown as { metadata: symbol }).metadata).toBe(metadataKey);
  });
});

describe('siteOf, standard', () => {
  test('reads placement, name, static, private and the class from the context', () => {
    const seen: ReturnType<typeof siteOf>[] = [];
    const watch = (value: unknown, context: StandardContext) => { seen.push(siteOf(value, context)); };
    @watch class Subject {
      @watch field = 1;
      @watch static shared = 2;
      @watch method() {}
      @watch #hidden() {}
    }
    const byName = (name: string | undefined) => seen.find(site => site.name === name)!;
    expect(byName('Subject').placement).toBe('class');
    expect(byName('Subject').ctor).toBe(Subject);
    expect(byName('field')).toMatchObject({ dialect: 'standard', placement: 'field', static: false, private: false, ctor: undefined });
    expect(byName('shared').static).toBe(true);
    expect(byName('method').placement).toBe('method');
    expect(byName('#hidden')).toMatchObject({ placement: 'method', private: true });
    expect(byName('method').metadata).toBeTypeOf('object');
  });
});

describe('records, standard', () => {
  test('a class with member decorators and no class decorator has its record', () => {
    const { store, field, method } = recorder();
    class Plain { @field a = 1; @field b = 2; @method run() {} }
    expect(store.of(Plain)).toEqual({ fields: ['a', 'b'], methods: ['run'] });
  });

  test('a class decorator and its members share one record', () => {
    const { store, field, tagged } = recorder();
    @tagged('x') class Both { @field a = 1; }
    expect(store.of(Both)).toEqual({ fields: ['a'], methods: [], tag: 'x' });
  });

  test('members never attach to another class', () => {
    const { store, field } = recorder();
    class First { @field one = 1; }
    class Second { @field two = 2; }
    expect(store.of(First)?.fields).toEqual(['one']);
    expect(store.of(Second)?.fields).toEqual(['two']);
  });

  test('a subclass has its own record, not its parent\'s', () => {
    const { store, field, tagged } = recorder();
    @tagged('parent') class Parent { @field inherited = 1; }
    class Undecorated extends Parent {}
    class Decorated extends Parent { @field own = 2; }
    expect(store.of(Undecorated)).toBe(undefined);
    expect(store.of(Decorated)?.fields).toEqual(['own']);
    expect(store.of(Decorated)?.tag).toBe(undefined);
    expect(store.of(Parent)).toEqual({ fields: ['inherited'], methods: [], tag: 'parent' });
  });

  test('two stores keep separate records for one class', () => {
    const a = recorder(); const b = recorder();
    class Shared { @a.field @b.method both() {} @a.field only = 1; }
    expect(a.store.of(Shared)).toEqual({ fields: ['both', 'only'], methods: [] });
    expect(b.store.of(Shared)).toEqual({ fields: [], methods: ['both'] });
  });

  test('an undecorated class has no record', () => {
    class Nothing {}
    expect(recorder().store.of(Nothing)).toBe(undefined);
  });

  test('without metadata, a class decorator still finds its record by class', () => {
    const store = createRecords<Notes>(make);
    class Bare {}
    const site = siteOf(Bare, { kind: 'class', name: 'Bare' } satisfies StandardContext);
    store.at(site).tag = 'bare';
    expect(store.of(Bare)?.tag).toBe('bare');
    expect(store.at(site)).toBe(store.of(Bare));
  });
});

describe('classChain', () => {
  test('lists a class and its bases, base first', () => {
    class A {} class B extends A {} class C extends B {}
    expect(classChain(C)).toEqual([A, B, C]);
  });
});
