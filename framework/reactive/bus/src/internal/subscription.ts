import type { DynHandler, IPubsub, InternalSubscription, Token } from '@coldmind/bus/src/types.ts'

export class InternalSub<TPayload, TMeta, TContext>
  implements InternalSubscription<TPayload, TMeta, TContext>
{
  private active = true

  constructor(
    private readonly router: any, // Use any to avoid circular dependency in types for now
    readonly id: string,
    readonly filter: string,
    readonly tokens: Token[],
    readonly handler: DynHandler<TPayload, TMeta, TContext>,
    readonly priority: number,
    readonly concurrency = 1,
  ) {}

  unsubscribe(): void {
    if (!this.active) return
    this.active = false
    this.router['removeSub'](this)
  }
}

export function createSubscriptionId(instanceId: string): string {
  const g = (globalThis as any)?.crypto?.randomUUID
  if (typeof g === 'function') {
    const uuid = g.call((globalThis as any).crypto)
    return `sub_${instanceId}_${uuid}`
  }
  const timestamp = Date.now().toString(36)
  const random = Math.floor(Math.random() * 1e12).toString(36)
  const random2 = Math.floor(Math.random() * 1e12).toString(36)
  return `sub_${instanceId}_${timestamp}_${random}_${random2}`
}

export function createMessageId(): string {
  const g = (globalThis as any)?.crypto?.randomUUID
  if (typeof g === 'function') return g.call((globalThis as any).crypto)
  return `dyn_${Date.now().toString(36)}_${Math.floor(Math.random() * 1e9).toString(36)}`
}

export function calculateMessageSize(payload: unknown, meta?: unknown): number {
  try {
    const payloadStr = JSON.stringify(payload)
    const metaStr = meta ? JSON.stringify(meta) : ''
    return payloadStr.length + metaStr.length
  } catch {
    return 0
  }
}
