import { describe, expect, test } from 'vitest'
import { check, checkAsync, int, model, obj, record, refine, refineAsync } from '@coldmind/model'

const Range = refine(model('range', { start: int(), end: int() }, { strict: true }), (value, ctx) =>
  value.start <= value.end ? null : { path: ctx.path, msg: 'Invalid range', code: 'RANGE', level: 'error' })
const AsyncRange = refineAsync(Range, async (value, ctx) => value.end < 100 ? null :
  { path: ctx.path, msg: 'Too far', code: 'LIMIT', level: 'error' })

describe('model validation metadata survives nested objects and records', () => {
  test('nested models retain strict mode', () => {
    const result = check({ range: obj(Range) }, { range: { start: 1, end: 2, extra: true } })
    expect(result.errors).toContainEqual({ path: ['range', 'extra'], msg: 'Unknown key', code: 'UNKNOWN_KEY', level: 'error' })
  })
  test('nested models run refinements on parsed values', () => {
    expect(check({ range: obj(Range) }, { range: { start: 3, end: 2 } }).errors[0]?.code).toBe('RANGE')
  })
  test('nested async refinements require the async entry point', async () => {
    const outer = { range: obj(AsyncRange) }
    expect(() => check(outer, { range: { start: 1, end: 100 } })).toThrow('async')
    expect((await checkAsync(outer, { range: { start: 1, end: 100 } })).errors[0]?.code).toBe('LIMIT')
  })
  test('record instances retain whole-model refinements', () => {
    const value = record(Range).create({ start: 3, end: 2 })
    expect(value.check().errors[0]?.code).toBe('RANGE')
  })
  test('record setters retain strict mode', () => {
    const value = record(Range).parse({ start: 1, end: 2 })
    expect(value.set({ start: 1, extra: true } as { start: number }).errors[0]?.code).toBe('UNKNOWN_KEY')
    expect(value.toJSON()).toEqual({ start: 1, end: 2 })
  })
  test('invalid refined patches leave records unchanged', () => {
    const value = record(Range).parse({ start: 1, end: 2 })
    expect(value.set({ start: 3 }).isValid).toBe(false)
    expect(value.start).toBe(1)
  })
  test('cloned records retain refinements', () => {
    const clone = record(Range).parse({ start: 1, end: 2 }).clone()
    clone.start = 3
    expect(clone.check().errors[0]?.code).toBe('RANGE')
  })
  test('async record checks and setters retain refinements', async () => {
    const value = await record(AsyncRange).parseAsync({ start: 1, end: 2 })
    expect(() => value.check()).toThrow('async')
    expect((await value.setAsync({ end: 100 })).errors[0]?.code).toBe('LIMIT')
    expect(value.end).toBe(2)
    value.end = 100
    expect((await value.checkAsync()).errors[0]?.code).toBe('LIMIT')
  })
})
