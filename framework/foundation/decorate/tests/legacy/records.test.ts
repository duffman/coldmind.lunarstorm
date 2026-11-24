import { describe, expect, test } from 'vitest';
import { createRecords, siteOf } from '@coldmind/decorate';

interface Notes { members: string[]; parameters: number[]; tag?: string }
const make = (): Notes => ({ members: [], parameters: [] });

function recorder() {
  const store = createRecords<Notes>(make);
  const member = (target: object, key: string | symbol, descriptor?: PropertyDescriptor) => {
    store.at(siteOf(target, key, descriptor)).members.push(String(key));
  };
  const parameter = (target: object, key: string | symbol | undefined, index: number) => {
    store.at(siteOf(target, key, index)).parameters.push(index);
  };
  const tagged = (tag: string) => (target: Function) => { store.at(siteOf(target)).tag = tag; };
  return { store, member, parameter, tagged };
}

describe('siteOf, legacy', () => {
  class Subject { field = 1; method() {} get read() { return 1; } set write(_: number) {} get both() { return 1; } set both(_: number) {} }
  const proto = Subject.prototype;
  const descriptor = (key: string) => Object.getOwnPropertyDescriptor(proto, key);

  test('tells every placement apart from the arguments', () => {
    expect(siteOf(Subject).placement).toBe('class');
    expect(siteOf(proto, 'field').placement).toBe('field');
    expect(siteOf(proto, 'method', descriptor('method')).placement).toBe('method');
    expect(siteOf(proto, 'read', descriptor('read')).placement).toBe('getter');
    expect(siteOf(proto, 'write', descriptor('write')).placement).toBe('setter');
    expect(siteOf(proto, 'both', descriptor('both')).placement).toBe('accessor');
    expect(siteOf(Subject, undefined, 0)).toMatchObject({ placement: 'parameter', index: 0, name: undefined });
    expect(siteOf(proto, 'method', 1)).toMatchObject({ placement: 'parameter', index: 1, name: 'method' });
  });

  test('resolves the class and the dialect', () => {
    expect(siteOf(Subject)).toMatchObject({ dialect: 'legacy', ctor: Subject, static: false, private: false, metadata: undefined });
    expect(siteOf(proto, 'field').ctor).toBe(Subject);
  });

  test('a member of the constructor is static', () => {
    expect(siteOf(Subject, 'shared').static).toBe(true);
    expect(siteOf(proto, 'field').static).toBe(false);
  });
});

describe('records, legacy', () => {
  test('member, parameter and class decorators share the class record', () => {
    const { store, member, parameter, tagged } = recorder();
    @tagged('x')
    class Subject {
      @member field = 1;
      constructor(@parameter first: number, @parameter second: number) { void first; void second; }
    }
    const record = store.of(Subject)!;
    expect(record.members).toEqual(['field']);
    expect([...record.parameters].sort()).toEqual([0, 1]);
    expect(record.tag).toBe('x');
  });

  test('members never attach to another class', () => {
    const { store, member } = recorder();
    class First { @member one = 1; }
    class Second { @member two = 2; }
    expect(store.of(First)?.members).toEqual(['one']);
    expect(store.of(Second)?.members).toEqual(['two']);
  });

  test('a subclass has its own record, not its parent\'s', () => {
    const { store, member } = recorder();
    class Parent { @member inherited = 1; }
    class Undecorated extends Parent {}
    class Decorated extends Parent { @member own = 2; }
    expect(store.of(Undecorated)).toBe(undefined);
    expect(store.of(Decorated)?.members).toEqual(['own']);
    expect(store.of(Parent)?.members).toEqual(['inherited']);
  });

  test('an undecorated class has no record', () => {
    class Nothing {}
    expect(recorder().store.of(Nothing)).toBe(undefined);
  });
});
