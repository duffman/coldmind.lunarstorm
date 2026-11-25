/**
 * Remembers message ids for a fixed window. Entries are kept in insertion order,
 * which is also expiry order, so trimming only ever looks at the front.
 */
export class DedupeWindow {
  private readonly seen = new Map<string, number>()

  constructor(
    private readonly windowMs: number,
    private readonly maxEntries: number,
  ) {}

  has(id: string): boolean {
    const expiresAt = this.seen.get(id)
    if (expiresAt === undefined) return false
    if (expiresAt > Date.now()) return true
    this.seen.delete(id)
    return false
  }

  add(id: string): void {
    this.seen.delete(id)
    this.seen.set(id, Date.now() + this.windowMs)
    const now = Date.now()
    for (const [key, expiresAt] of this.seen) {
      if (expiresAt > now && this.seen.size <= this.maxEntries) break
      this.seen.delete(key)
    }
  }

  delete(id: string): void {
    this.seen.delete(id)
  }

  clear(): void {
    this.seen.clear()
  }

  get size(): number {
    return this.seen.size
  }
}
