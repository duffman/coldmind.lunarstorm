import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { test } from 'node:test';
import * as api from '@coldmind/decorate';

const names = ['core', 'metadata', 'records', 'site'];

test('Node resolves compiled entries and preserves the public runtime API', () => {
  assert.match(import.meta.resolve('@coldmind/decorate'), /\/dist\/decorate\.core\.js$/);
  assert.deepEqual(Object.keys(api).sort(), ['classChain', 'createRecords', 'metadataKey', 'placementPhrase', 'siteOf']);
  assert.equal(Symbol.metadata, api.metadataKey);
  class Base {}
  class Child extends Base {}
  assert.deepEqual(api.classChain(Child), [Base, Child]);
  const records = api.createRecords(() => ({ names: [] }));
  records.at(api.siteOf(Child.prototype, 'field')).names.push('field');
  assert.deepEqual(records.of(Child), { names: ['field'] });
  assert.equal(records.of(Base), undefined);
  const metadata = {};
  records.at(api.siteOf(undefined, { kind: 'field', name: 'standard', metadata })).names.push('standard');
  class Standard {}
  Object.defineProperty(Standard, api.metadataKey, { value: metadata });
  assert.deepEqual(records.of(Standard), { names: ['standard'] });
});

test('explicit internal and old source entries resolve to the same compiled modules', async () => {
  for (const name of names) {
    const canonical = `@coldmind/decorate/internal/${name}`;
    const compatibility = `@coldmind/decorate/src/decorate.${name}.ts`;
    assert.match(import.meta.resolve(canonical), /\/dist\/[^/]+\.js$/);
    assert.equal(import.meta.resolve(compatibility), import.meta.resolve(canonical));
    assert.equal(await import(canonical), await import(compatibility));
    const emitted = await readFile(new URL(import.meta.resolve(canonical)), 'utf8');
    assert.doesNotMatch(emitted, /@coldmind\/decorate\/src\//);
  }
});

test('distribution exports order conditions and ships metadata side effects', async () => {
  const pkg = JSON.parse(await readFile(new URL('../../package.json', import.meta.url), 'utf8'));
  assert.ok(pkg.files.includes('dist'));
  assert.ok(pkg.sideEffects.includes('./dist/decorate.metadata.js'));
  for (const [key, value] of Object.entries(pkg.exports)) {
    if (key === './package.json') continue;
    assert.deepEqual(Object.keys(value), ['types', 'bun', 'default']);
    assert.match(value.types, /^\.\/dist\/.*\.d\.ts$/);
    assert.match(value.bun, /^\.\/src\/.*\.ts$/);
    assert.match(value.default, /^\.\/dist\/.*\.js$/);
    await readFile(new URL(`../../${value.types}`, import.meta.url));
    await readFile(new URL(`../../${value.default}`, import.meta.url));
  }
});
