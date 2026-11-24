import { describe, expect, it } from 'vitest'
import { formatErrors, formatPath, type ValidationError } from '@coldmind/core'

const error = (path: ValidationError['path'], msg: string): ValidationError => ({ path, msg, code: 'X', level: 'error' })

describe('validation results', () => {
  it('formats a path with keys and indexes', () => {
    expect(formatPath(['items', 0, 'qty'])).toBe('items[0].qty')
  })

  it('formats every error on one line, the root as (root)', () => {
    expect(formatErrors([error(['email'], 'Invalid email'), error([], 'Required')])).toBe('email: Invalid email; (root): Required')
  })
})
