import { describe, expect, test } from 'bun:test';
import { ProjectStore } from './project-store';

describe('ProjectStore', () => {
  test('editing a draft does not change the stored project until save', () => {
    const store = new ProjectStore();
    const draft = store.read('P-001');
    draft.name = 'Updated name';
    expect(store.read('P-001').name).toBe('Desktop component library');
    store.save(draft);
    expect(store.read('P-001').name).toBe('Updated name');
  });

  test('validation rejects empty names without changing stored data', () => {
    const store = new ProjectStore();
    const draft = store.read('P-001');
    draft.name = '  ';
    expect(() => store.save(draft)).toThrow('Project name is required.');
    expect(store.read('P-001').name).toBe('Desktop component library');
  });

  test('new projects receive unique identifiers and lists are snapshots', () => {
    const store = new ProjectStore();
    const first = store.create();
    const second = store.create();
    expect(first.id).not.toBe(second.id);
    const list = store.list();
    list[0]!.name = 'Changed outside the store';
    expect(store.read(list[0]!.id).name).not.toBe('Changed outside the store');
  });
});
