/** Preparation only: imports qualify from an isolated token copy, not the public API. */
import { expect, test } from 'vitest';
import { Container, Inject, Injectable, describe, tokenName as liveTokenName, NotRegisteredError } from '@coldmind/inject';
import {
  qualify, createToken, normalizeToken, internToken, lookupToken, retainTokenKey,
  releaseTokenKey, getTokenCacheSize, clearTokenCaches, tokenName, delay,
} from '@coldmind/inject/token';

class Db { constructor(readonly role = 'default') {} }

test('same base and qualifier are interned symbols, distinct from their base', () => {
  const replica = qualify(Db, 'replica');
  expect(typeof replica).toBe('symbol');
  expect(qualify(Db, 'replica')).toBe(replica);
  expect(replica).not.toBe(normalizeToken(Db));
});

test('different qualifiers, bases, and identically described symbols remain distinct', () => {
  class OtherDb {}
  expect(qualify(Db, 'replica')).not.toBe(qualify(Db, 'primary'));
  expect(qualify(Db, 'replica')).not.toBe(qualify(OtherDb, 'replica'));
  expect(qualify(Symbol('Db'), 'replica')).not.toBe(qualify(Symbol('Db'), 'replica'));
  expect(qualify('Db', 'replica')).not.toBe(qualify(Db, 'replica'));
});

test('class, symbol, and normalized class bases share semantic identity', () => {
  const service = createToken<Db>('typed-db');
  expect(qualify(Db, 'replica')).toBe(qualify(normalizeToken(Db), 'replica'));
  expect(qualify(service, 'replica')).toBe(qualify(normalizeToken(service), 'replica'));
});

test('raw strings and their publicly normalized keys share qualification identity', () => {
  const replica = qualify('qualified-public-string', 'replica');
  const key = normalizeToken('qualified-public-string');
  expect(qualify(key, 'replica')).toBe(replica);
  expect(qualify(key, 'primary')).not.toBe(replica);
});

test('delayed classes resolve at qualification time and share direct class identity', () => {
  let reads = 0;
  const delayed = delay(() => { reads++; return Db; });
  const replica = qualify(delayed, 'replica');
  expect(reads).toBe(1);
  expect(replica).toBe(qualify(Db, 'replica'));
  expect(normalizeToken(replica)).toBe(replica);
  expect(reads).toBe(1);
});

test('qualifying an absent string does not allocate or pin its underlying registry key', () => {
  const before = getTokenCacheSize();
  const replica = qualify('qualified-absent-string', 'replica');
  expect(getTokenCacheSize()).toBe(before);
  expect(lookupToken('qualified-absent-string')).toBeUndefined();
  clearTokenCaches();
  expect(qualify('qualified-absent-string', 'replica')).toBe(replica);
});

test('internal normalized strings remain collectible and recreate the same derived token', () => {
  const before = getTokenCacheSize();
  const key = internToken('qualified-retained-string');
  expect(retainTokenKey(key)).toBe(true);
  const replica = qualify(key, 'replica');
  expect(qualify('qualified-retained-string', 'replica')).toBe(replica);
  clearTokenCaches(); expect(lookupToken('qualified-retained-string')).toBe(key);
  releaseTokenKey(key); clearTokenCaches();
  expect(getTokenCacheSize()).toBe(before);
  expect(lookupToken('qualified-retained-string')).toBeUndefined();
  const replacement = internToken('qualified-retained-string');
  expect(replacement).not.toBe(key);
  expect(qualify(replacement, 'replica')).toBe(replica);
  releaseTokenKey(replacement);
});

test('public base pins and derived symbols survive collection independently', () => {
  const key = normalizeToken('qualified-pinned-string');
  const replica = qualify(key, 'replica');
  clearTokenCaches();
  expect(lookupToken('qualified-pinned-string')).toBe(key);
  expect(qualify('qualified-pinned-string', 'replica')).toBe(replica);
  expect(normalizeToken(replica)).toBe(replica);
});

test('empty and delimiter-containing qualifiers are not concatenation identity keys', () => {
  expect(qualify('a#b', 'c')).not.toBe(qualify('a', 'b#c'));
  expect(qualify(Db, '')).toBe(qualify(Db, ''));
  expect(qualify(Db, '')).not.toBe(qualify(Db, 'replica'));
});

test('two qualified bindings resolve independently without registering the base', async () => {
  const root = new Container(); const primary = new Db('primary'); const replica = new Db('replica');
  try {
    root.registerInstance(qualify(Db, 'primary'), primary);
    root.registerInstance(qualify(Db, 'replica'), replica);
    expect(await root.resolve(qualify(Db, 'primary'))).toBe(primary);
    expect(root.resolveSync(qualify(Db, 'replica'))).toBe(replica);
    expect(root.has(Db)).toBe(false);
  } finally { await root.dispose(); }
});

test('decorated constructor and field accept qualified symbols unchanged', async () => {
  const replica = qualify(Db, 'replica');
  class Consumer {
    field!: Db;
    constructor(readonly constructorDb: Db) {}
  }
  // Invoke the legacy signatures directly so this draft needs no live tsconfig changes.
  Inject(replica)(Consumer, undefined, 0);
  Inject(replica)(Consumer.prototype, 'field');
  Injectable()(Consumer);
  expect(describe(Consumer).params[0]?.token).toBe(replica);
  expect(describe(Consumer).members[0]?.token).toBe(replica);
  const root = new Container(); const db = new Db('replica');
  try {
    root.registerInstance(replica, db); root.registerClass(Consumer);
    const consumer = await root.resolve(Consumer);
    expect(consumer.constructorDb).toBe(db); expect(consumer.field).toBe(db);
  } finally { await root.dispose(); }
});

test('graph nodes and declared dependency edges preserve readable qualified names', async () => {
  const root = new Container(); const primary = qualify(Db, 'primary'); const replica = qualify(Db, 'replica');
  try {
    root.singleton(primary, () => new Db('primary'));
    root.singleton(replica, () => new Db('replica'));
    root.transient('qualified-consumer', c => c.resolveSync(replica), { deps: [replica] });
    const graph = root.graph();
    expect(graph.nodes().map(node => liveTokenName(node.token))).toContain('Db#primary');
    expect(graph.nodes().map(node => liveTokenName(node.token))).toContain('Db#replica');
    expect(graph.dependencies(root.getRegisteredTokens()[2]!).map(edge => liveTokenName(edge.token))).toEqual(['Db#replica']);
    expect(() => root.validate()).not.toThrow();
  } finally { await root.dispose(); }
});

test('missing-binding diagnostics distinguish qualification names', async () => {
  const root = new Container();
  try {
    expect(() => root.resolveSync(qualify(Db, 'replica'))).toThrow(NotRegisteredError);
    expect(() => root.resolveSync(qualify(Db, 'replica'))).toThrow('Db#replica');
    expect(() => root.resolveSync(qualify(Db, 'primary'))).toThrow('Db#primary');
  } finally { await root.dispose(); }
});

test('tokenName renders qualified class, string, typed and anonymous symbol bases', () => {
  expect(tokenName(qualify(Db, 'replica'))).toBe('Db#replica');
  expect(tokenName(qualify('db-name', 'replica'))).toBe('db-name#replica');
  expect(tokenName(qualify(createToken<Db>('typed-db'), 'replica'))).toBe('typed-db#replica');
  expect(tokenName(qualify(Symbol(), 'replica'))).toBe('#replica');
});
