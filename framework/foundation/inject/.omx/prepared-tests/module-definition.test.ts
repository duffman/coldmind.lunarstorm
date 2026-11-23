import { expect, test } from 'vitest';
import { composeModules, defineModule, type ModuleDefinition } from '@coldmind/inject/src/core/cminject.module.ts';

test('defineModule preserves definition, registration and import identities without executing them', () => {
  let calls = 0;
  const imports: ModuleDefinition[] = [];
  const definition = { name: 'identity', imports, register: () => { calls++; } };
  const module = defineModule(definition);
  expect(module).toBe(definition);
  expect(module.imports).toBe(imports);
  expect(module.register).toBe(definition.register);
  expect(calls).toBe(0);
});

test('typed declarations have no runtime phantom properties or registration side effects', () => {
  const definition = { name: 'typed', register: async () => {} };
  const module = defineModule<{ Db: { query(): string } }, { Config: string }>()(definition);
  expect(module).toBe(definition);
  expect(Reflect.ownKeys(module)).toEqual(['name', 'register']);
});

test('definitions retain mutable import references so the loader can diagnose actual cycles', () => {
  const imports: ModuleDefinition[] = [];
  const first = defineModule({ name: 'first', imports, register: () => {} });
  const second = defineModule({ name: 'second', imports: [first], register: () => {} });
  imports.push(second);
  expect(first.imports![0]).toBe(second);
  expect(second.imports![0]).toBe(first);
});

test('definitions do not merge modules with equal diagnostic names', () => {
  const first = defineModule({ name: 'duplicate', register: () => {} });
  const second = defineModule({ name: 'duplicate', register: () => {} });
  expect(first).not.toBe(second);
});

test('composition preserves import order and identity without loading registrations', () => {
  let calls = 0;
  const first = defineModule({ name: 'first', register: () => { calls++; } });
  const second = defineModule({ name: 'second', register: () => { calls++; } });
  const composition = composeModules(first, second);
  expect(composition.name).toBe('first+second');
  expect(composition.imports).toEqual([first, second]);
  expect(composition.imports![0]).toBe(first);
  expect(composition.imports![1]).toBe(second);
  expect(calls).toBe(0);
});

test('empty composition is a valid registration-free module', () => {
  const composition = composeModules();
  expect(composition.name).toBe('composition');
  expect(composition.imports).toEqual([]);
  expect(composition.register({} as never)).toBeUndefined();
});

test.each(['', '  '])('invalid diagnostic names are rejected (%s)', name => {
  expect(() => defineModule({ name, register: () => {} })).toThrow('Module name required');
});

test('malformed registration and imports fail before loader integration', () => {
  expect(() => defineModule({ name: 'bad-register', register: 1 } as never)).toThrow('Module register required: bad-register');
  expect(() => defineModule({ name: 'bad-imports', imports: {}, register: () => {} } as never)).toThrow('Module imports must be an array: bad-imports');
});
