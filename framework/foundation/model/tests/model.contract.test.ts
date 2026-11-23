import { describe, expect, test } from 'vitest'
import {
  any,
  bool,
  check,
  checkAsync,
  defineDataType,
  discriminated,
  int,
  introspect,
  list,
  listOf,
  makeOptional,
  makeRequired,
  merge,
  model,
  num,
  obj,
  omit,
  parse,
  partial,
  pick,
  publicView,
  refine,
  refineAsync,
  record,
  str,
  toJsonSchema,
  union,
  ValidationFailed,
  type Model,
} from '@coldmind/model'
import { buildModel, MODEL as INTERNAL_MODEL } from '@coldmind/model/internal'
import { MODEL } from '@coldmind/model/model.key'

const Account = model('account', { id: int(), email: str().trim().email() })
const strictShape = { x: int() }
const strictModel = () => buildModel('s', strictShape, true) as unknown as Model<'s', typeof strictShape>

// ── fields ──────────────────────────────────────────────────

describe('fields', () => {
  test('required by default; optional and nullable widen', () => {
    expect(str().validateSync([], undefined).errors[0]?.code).toBe('REQ')
    expect(str().optional().validateSync([], undefined).isValid).toBe(true)
    expect(str().validateSync([], null).errors[0]?.code).toBe('NULL')
    expect(str().nullable().validateSync([], null).value).toBeNull()
  })

  test('default fills undefined; factory is called per use', () => {
    expect(int().default(3).validateSync([], undefined).value).toBe(3)
    let n = 0
    const f = int().default(() => ++n)
    expect([f.validateSync([], undefined).value, f.validateSync([], undefined).value]).toEqual([1, 2])
  })

  test('modifiers are immutable', () => {
    const base = str()
    base.min(3)
    expect(base.validateSync([], 'a').isValid).toBe(true)
  })

  test('transform runs before constraints', () => {
    expect(str().trim().min(2).validateSync([], '  a ').errors[0]?.code).toBe('MIN')
    expect(str().trim().lowercase().validateSync([], ' AB ').value).toBe('ab')
  })

  test('type is strict unless coerced', () => {
    expect(bool().validateSync([], 'true').errors[0]?.code).toBe('TYPE')
    expect(num().validateSync([], Number.NaN).errors[0]?.code).toBe('TYPE')
    expect(num().coerceNumber().validateSync([], '12').value).toBe(12)
  })

  test('coerceNumber does not turn blank strings into 0', () => {
    expect(num().coerceNumber().validateSync([], '').isValid).toBe(false)
    expect(num().coerceNumber().validateSync([], '   ').isValid).toBe(false)
  })

  test('pattern with a global regex is stable across calls', () => {
    const f = str().pattern(/ab/g)
    expect([1, 2, 3].map(() => f.validateSync([], 'ab').isValid)).toEqual([true, true, true])
  })

  test('int, oneOf, email', () => {
    expect(int().validateSync([], 1.5).errors[0]?.code).toBe('INTEGER')
    expect(str().oneOf(['a', 'b']).validateSync([], 'c').errors[0]?.code).toBe('ONE_OF')
    expect(str().email().validateSync([], 'a@b.co').isValid).toBe(true)
  })

  test('list reports item paths and length', () => {
    const tags = list(str().nonempty()).maxLength(2)
    expect(tags.validateSync(['t'], ['a', '']).errors[0]?.path).toEqual(['t', 1])
    expect(tags.validateSync([], ['a', 'b', 'c']).errors[0]?.code).toBe('MAX_LENGTH')
  })

  test('obj drops unknown keys with a warning; strict makes them errors', () => {
    const r = obj({ a: int() }).validateSync([], { a: 1, b: 2 })
    expect(r.value).toEqual({ a: 1 })
    expect(r.warnings[0]?.code).toBe('UNKNOWN_KEY')
    expect(obj({ a: int() }).strict().validateSync([], { a: 1, b: 2 }).isValid).toBe(false)
  })

  test('union: first matching variant wins', () => {
    const u = union(int(), str())
    expect(u.validateSync([], 'x').value).toBe('x')
    expect(u.validateSync([], true).errors[0]?.code).toBe('UNION')
  })

  test('union takes presence from its variants', () => {
    expect(union(str().optional(), int()).validateSync([], undefined).isValid).toBe(true)
    expect(union(str().nullable(), int()).validateSync([], null).value).toBeNull()
    expect(union(str(), int().default(7)).validateSync([], undefined).value).toBe(7)
    expect(union(str(), int()).validateSync([], undefined).errors[0]?.code).toBe('REQ')
    expect(union(str(), int()).validateSync([], null).errors[0]?.code).toBe('NULL')
    expect(union(str().optional(), int()).required().validateSync([], undefined).isValid).toBe(false)
    expect(check(model({ u: union(str().optional(), int()) }), {}).isValid).toBe(true)
  })

  test('discriminated selects by tag', () => {
    const D = discriminated('kind', { cat: { lives: int() }, dog: { bark: str() } })
    expect(D.validateSync([], { kind: 'cat', lives: 9 }).isValid).toBe(true)
    expect(D.validateSync([], { kind: 'cow' }).errors[0]?.code).toBe('DISCRIMINANT')
  })

  test('discriminated rejects inherited names as tags instead of crashing', () => {
    const D = discriminated('kind', { cat: { lives: int() } })
    for (const kind of ['toString', 'constructor', '__proto__']) {
      expect(D.validateSync([], { kind }).errors[0]?.code).toBe('DISCRIMINANT')
    }
  })

  test('async validators need the async path', async () => {
    const f = str().useAsync(async (v, ctx) =>
      v === 'taken' ? { path: ctx.path, msg: 'taken', code: 'TAKEN', level: 'error' } : null,
    )
    expect(() => f.validateSync([], 'x')).toThrow()
    expect((await f.validate([], 'taken')).errors[0]?.code).toBe('TAKEN')
  })

  test('modifiers on containers share children', () => {
    const o = obj({ a: int() })
    expect(o.optional().shape.a).toBe(o.shape.a)
    const l = list(str())
    expect(l.nullable().item).toBe(l.item)
  })

  test('default() keeps the subclass', () => {
    const tags = list(str()).default([]).maxLength(2)
    expect(tags.validateSync([], undefined).value).toEqual([])
    expect(tags.validateSync([], ['a', 'b', 'c']).errors[0]?.code).toBe('MAX_LENGTH')
    const o = obj({ a: int() }).default({ a: 1 }).strict()
    expect(o.validateSync([], { a: 1, b: 2 }).isValid).toBe(false)
    const D = discriminated('kind', { cat: { lives: int() } }).optional()
    expect(D.validateSync([], undefined).isValid).toBe(true)
  })

  test('hidden sets the flag and leaves validation alone', () => {
    const f = str().hidden()
    expect(f._opts.hidden).toBe(true)
    expect(f.validateSync([], 'x').isValid).toBe(true)
  })

  test('hidden state survives later presence modifiers', () => {
    expect(Object.keys(publicView(model({ a: str().hidden().optional(), b: str() })))).toEqual(['b'])
    expect(Object.keys(publicView(model({ a: str().optional().hidden(), b: str() })))).toEqual(['b'])
  })
})

// ── models and projections ──────────────────────────────────

describe('models', () => {
  test('handles carry model, key and field', () => {
    expect(Account.email.model).toBe('account')
    expect(Account.email.key).toBe('email')
    expect(Object.keys(Account)).toEqual(['id', 'email'])
  })

  test('internal re-exports the same MODEL symbol', () => {
    expect(INTERNAL_MODEL).toBe(MODEL)
    expect((Account as unknown as Record<symbol, unknown>)[INTERNAL_MODEL]).toBeDefined()
  })

  test('pick, omit, partial, makeOptional, makeRequired, merge', () => {
    expect(Object.keys(pick(Account, Account.email))).toEqual(['email'])
    expect(Object.keys(omit(Account, Account.id))).toEqual(['email'])
    expect(check(partial(Account), {}).isValid).toBe(true)
    const Signup = makeOptional(Account, Account.id)
    expect(check(Signup, { email: 'a@b.co' }).isValid).toBe(true)
    const Patch = partial(Account)
    expect(check(makeRequired(Patch, Patch.email!), {}).isValid).toBe(false)
    expect(Object.keys(merge(Account, { name: str() }))).toEqual(['id', 'email', 'name'])
  })

  test('handle projections throw on a foreign key', () => {
    const foreign = { model: 'account', key: 'nope', field: str() } as never
    expect(() => pick(Account, foreign)).toThrow('pick()')
    expect(() => omit(Account, foreign)).toThrow('omit()')
    expect(() => makeOptional(Account, foreign)).toThrow('makeOptional()')
    expect(() => makeRequired(Account, foreign)).toThrow('makeRequired()')
  })

  test('handle projections reject a same-name same-key handle from another model', () => {
    const Other = model('account', { id: int(), email: str().trim().email() })
    expect(() => pick(Account, Other.id as never)).toThrow('does not belong')
  })

  test('models snapshot their field map', () => {
    const fields = { x: int() }
    const M = model('snapshot', fields)
    fields.x = str() as never
    expect(check(M, { x: 1 }).isValid).toBe(true)
    expect(check(M, { x: 'changed' }).isValid).toBe(false)
  })

  test('model() accepts strict mode directly', () => {
    const S = model('strict', { x: int() }, { strict: true })
    expect(check(S, { x: 1, y: 2 }).isValid).toBe(false)
  })

  test('partial suppresses defaults', () => {
    const M = model({ id: int().default(7), name: str().default('x') })
    expect(parse(partial(M), {})).toEqual({})
  })

  test('projections keep strict', () => {
    const S = strictModel()
    expect(check(S, { x: 1, y: 2 }).isValid).toBe(false)
    expect(check(pick(S, S.x), { x: 1, y: 2 }).isValid).toBe(false)
    expect(check(omit(S), { x: 1, y: 2 }).isValid).toBe(false)
    expect(check(partial(S), { x: 1, y: 2 }).isValid).toBe(false)
    expect(check(makeOptional(S, S.x), { x: 1, y: 2 }).isValid).toBe(false)
    expect(check(makeRequired(S, S.x), { x: 1, y: 2 }).isValid).toBe(false)
    expect(check(merge(S, { z: int() }), { x: 1, z: 1, y: 2 }).isValid).toBe(false)
  })

  test('publicView drops hidden fields and keeps strict', () => {
    const U = model('u', { name: str(), password: str().hidden() })
    const P = publicView(U)
    expect(Object.keys(P)).toEqual(['name'])
    expect(introspect(U).fields.password?.hidden).toBe(true)
    expect(introspect(U).fields.name?.hidden).toBe(false)
    const S = buildModel('s', { x: int(), y: int().hidden() }, true)
    expect(check(publicView(S), { x: 1, y: 2 }).isValid).toBe(false)
  })

  test('listOf wraps items', () => {
    expect(check(listOf(Account), { items: [{ id: 1, email: 'a@b.co' }] }).isValid).toBe(true)
  })
})

// ── validation verbs ────────────────────────────────────────

describe('check / parse', () => {
  test('check returns a result; parse returns the value or throws', () => {
    expect(check(Account, { id: 1, email: ' a@b.co ' }).value).toEqual({ id: 1, email: 'a@b.co' })
    expect(() => parse(Account, { id: 'x' })).toThrow(ValidationFailed)
  })

  test('non-object input', () => {
    expect(check(Account, 5).isValid).toBe(false)
    expect(check(Account, [1]).isValid).toBe(false)
  })

  test('top-level input gets the same codes as a nested obj()', () => {
    expect(check(Account, 5).errors[0]?.code).toBe('TYPE')
    expect(check(Account, undefined).errors[0]?.code).toBe('REQ')
    expect(check(Account, null).errors[0]?.code).toBe('NULL')
    const nested = check(model({ a: obj(Account) }), { a: 5 }).errors[0]
    expect(nested?.code).toBe('TYPE')
    expect(nested?.path).toEqual(['a'])
  })

  test('strict option turns unknown keys into errors', () => {
    expect(check(Account, { id: 1, email: 'a@b.co', x: 1 }).isValid).toBe(true)
    expect(check(Account, { id: 1, email: 'a@b.co', x: 1 }, { strict: true }).isValid).toBe(false)
  })

  test('strict catches inherited names used as keys', () => {
    const M = model({ x: int() })
    expect(check(M, JSON.parse('{"x":1,"__proto__":{"p":1}}'), { strict: true }).isValid).toBe(false)
    expect(check(M, { x: 1, constructor: 1 }, { strict: true }).isValid).toBe(false)
  })

  test('a missing field is REQ even when its name exists on Object.prototype', () => {
    expect(check(model({ toString: str() }), {}).errors[0]?.code).toBe('REQ')
  })

  test('checkAsync matches check on sync shapes', async () => {
    expect((await checkAsync(Account, { id: 1, email: 'a@b.co' })).isValid).toBe(true)
  })

  test('whole-model refinements run after field parsing', () => {
    const Range = refine(model('range', { start: int(), end: int() }), (v, ctx) =>
      v.start < v.end
        ? null
        : { path: ctx.path, msg: 'start must be before end', code: 'RANGE', level: 'error' },
    )
    expect(check(Range, { start: 1, end: 2 }).isValid).toBe(true)
    expect(check(Range, { start: 2, end: 1 }).errors[0]?.code).toBe('RANGE')
  })

  test('async whole-model refinements make the model async-only', async () => {
    const M = refineAsync(model({ a: int() }), async (v, ctx) =>
      v.a === 1 ? null : { path: ctx.path, msg: 'bad', code: 'ROOT', level: 'error' },
    )
    expect(() => check(M, { a: 1 })).toThrow()
    expect((await checkAsync(M, { a: 2 })).errors[0]?.code).toBe('ROOT')
  })
})

// ── records ─────────────────────────────────────────────────

describe('record', () => {
  const User = record(model('user', { name: str().nonempty(), age: int().optional() }))

  test('create, property access, set validates before applying', () => {
    const u = User.create({ name: 'Ada' })
    expect(u.name).toBe('Ada')
    expect(u.set({ name: '' }).isValid).toBe(false)
    expect(u.name).toBe('Ada')
    u.set({ age: 36 })
    expect(u.toJSON()).toEqual({ name: 'Ada', age: 36 })
  })

  test('parse throws on invalid data', () => {
    expect(() => User.parse({ name: '' })).toThrow(ValidationFailed)
  })

  test('a field named like a record member is rejected', () => {
    expect(() => record(model('r', { set: str(), shape: str() }))).toThrow('set, shape')
    expect(() => record(model('r', { toJSON: str() }))).toThrow()
  })

  test('literal defaults are not shared between instances', () => {
    const L = record(model('l', { tags: any<string[]>().default([]) }))
    const a = L.create()
    a.tags.push('x')
    expect(L.create().tags).toEqual([])
  })

  test('nullable does not mean absent', () => {
    const R = record(model('nullable', { value: str().nullable() }))
    expect(R.create().toJSON() as Record<string, unknown>).toEqual({})
    expect(R.create().check().errors[0]?.code).toBe('REQ')
    expect(R.create({ value: null }).check().isValid).toBe(true)
  })

  test('data types can preserve class instances while cloning', () => {
    class Money {
      constructor(readonly cents: number) {}
    }
    const money = defineDataType({
      name: `money-${Math.random()}`,
      base: () => any<Money>().use((v, ctx) =>
        v instanceof Money ? null : { path: ctx.path, msg: 'money', code: 'TYPE', level: 'error' },
      ),
      clone: (v: Money) => new Money(v.cents),
    })
    const R = record(model('money', { total: money().default(new Money(100)) }))
    const a = R.create()
    const b = a.clone()
    expect(a.total).toBeInstanceOf(Money)
    expect(b.total).toBeInstanceOf(Money)
    expect(b.total).not.toBe(a.total)
  })
})

// ── introspection and JSON Schema ───────────────────────────

describe('introspect / toJsonSchema', () => {
  test('introspect lists fields in order', () => {
    const info = introspect(Account)
    expect(info.name).toBe('account')
    expect(info.order).toEqual(['id', 'email'])
    expect(info.fields.id?.kind).toBe('number')
  })

  test('schema types, required and constraints', () => {
    const s = toJsonSchema(model('t', { id: int(), tag: str().max(5).optional() }))
    expect(s.properties?.id?.type).toBe('integer')
    expect(s.properties?.tag?.maxLength).toBe(5)
    expect(s.required).toEqual(['id'])
  })

  test('repeated constraints combine; flagged patterns are left out', () => {
    expect(toJsonSchema(str().min(2).min(5).max(9).max(7))).toMatchObject({ minLength: 5, maxLength: 7 })
    expect(toJsonSchema(str().oneOf(['a', 'b', 'c']).oneOf(['b', 'c', 'd'])).enum).toEqual(['b', 'c'])
    const two = toJsonSchema(str().pattern(/^a/).pattern(/z$/g))
    expect(two.pattern).toBe('^a')
    expect(two.allOf).toEqual([{ pattern: 'z$' }])
    expect(toJsonSchema(str().pattern(/abc/i)).pattern).toBeUndefined()
  })

  test('repeated list bounds and formats preserve conjunction semantics', () => {
    expect(toJsonSchema(list(str()).minLength(10).minLength(1)).minItems).toBe(10)
    expect(toJsonSchema(list(str()).maxLength(2).maxLength(9)).maxItems).toBe(2)
    const formats = toJsonSchema(str().email().url())
    expect(formats.format).toBe('email')
    expect(formats.allOf).toContainEqual({ format: 'uri' })
  })

  test('union metadata and discriminant constraints survive schema generation', () => {
    const U = toJsonSchema(union(str(), int()).doc('value').default('x'))
    expect(U.description).toBe('value')
    expect(U.default).toBe('x')

    const D = toJsonSchema(
      discriminated('kind', { cat: { kind: str().doc('kind docs'), lives: int() } }),
    )
    const kind = D.oneOf?.[0]?.properties?.kind
    expect(kind?.allOf?.[0]?.description).toBe('kind docs')
    expect(kind?.allOf?.[1]?.const).toBe('cat')
  })

  test('discriminated introspection exposes tags without overwriting docs', () => {
    const D = discriminated('kind', { cat: { lives: int() }, dog: { bark: str() } })
    const info = introspect(model({ animal: D })).fields.animal
    expect(info?.variantTags).toEqual(['cat', 'dog'])
  })

  test('schema of a strict model forbids additional properties', () => {
    expect(toJsonSchema(strictModel()).additionalProperties).toBe(false)
  })

  test('a model with a field named validateSync is still a model', () => {
    expect(toJsonSchema(model({ validateSync: str() })).type).toBe('object')
  })
})
