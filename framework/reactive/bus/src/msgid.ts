import { randomUUID } from 'node:crypto'

export function msgId(prefix = 'msg'): string {
  return `${prefix}_${randomUUID()}`
}
