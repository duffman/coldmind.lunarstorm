import { expect, test } from 'vitest';
import { Container, LifecycleError } from '@coldmind/inject';

test('resolve always returns a rejected promise for an unshieldable thenable', async () => {
  const root = new Container(); const value = Object.freeze({ then: () => undefined }); root.singleton('promise-frozen', () => value);
  try {
    let pending!: Promise<unknown>; expect(() => { pending = root.resolve('promise-frozen'); }).not.toThrow();
    await expect(pending).rejects.toBeInstanceOf(LifecycleError); expect(root.resolveSync('promise-frozen')).toBe(value); expect(root.get('promise-frozen')).toBe(value);
  } finally { await root.dispose(); }
});

test('instantiateAsync returns a rejection rather than throwing when completion yields a frozen thenable', async () => {
  /** An object whose then property cannot be hidden by promise resolution. */
  class Frozen { constructor() { return Object.freeze({ then: () => undefined }) as Frozen; } }
  const root = new Container();
  try {
    let pending!: Promise<Frozen>; expect(() => { pending = root.instantiateAsync(Frozen); }).not.toThrow();
    await expect(pending).rejects.toBeInstanceOf(LifecycleError);
  } finally { await root.dispose(); }
});

test('injectMembersAsync returns a rejection rather than throwing for a frozen thenable', async () => {
  const root = new Container(); const value = Object.freeze({ then: () => undefined });
  try {
    let pending!: Promise<typeof value>; expect(() => { pending = root.injectMembersAsync(value); }).not.toThrow();
    await expect(pending).rejects.toBeInstanceOf(LifecycleError); expect(root.injectMembers(value)).toBe(value);
  } finally { await root.dispose(); }
});
