import { DynInvalidFilterError, DynInvalidTopicError } from '@coldmind/bus/src/errors.ts'
import type { InternalSubscription, Token } from '@coldmind/bus/src/types.ts'

type Edge = { kind: 'literal'; key: string } | { kind: 'plus' } | { kind: 'root' }

class TrieNode<TPayload, TMeta, TContext> {
  literals = new Map<string, TrieNode<TPayload, TMeta, TContext>>()
  plus?: TrieNode<TPayload, TMeta, TContext>
  handlers = new Map<string, InternalSubscription<TPayload, TMeta, TContext>>()
  hashHandlers = new Map<string, InternalSubscription<TPayload, TMeta, TContext>>()
  parent?: TrieNode<TPayload, TMeta, TContext>
  edgeFromParent: Edge = { kind: 'root' }

  constructor(parent?: TrieNode<TPayload, TMeta, TContext>, edge: Edge = { kind: 'root' }) {
    this.parent = parent
    this.edgeFromParent = edge
  }

  isEmpty(): boolean {
    return (
      this.handlers.size === 0 &&
      this.hashHandlers.size === 0 &&
      this.literals.size === 0 &&
      this.plus === undefined
    )
  }
}

export class PatternMatcher<TPayload, TMeta, TContext> {
  private readonly root = new TrieNode<TPayload, TMeta, TContext>()

  addSubscription(sub: InternalSubscription<TPayload, TMeta, TContext>): void {
    let node = this.root

    for (let i = 0; i < sub.tokens.length; i++) {
      const t = sub.tokens[i]

      if (t === '#') {
        node.hashHandlers.set(sub.id, sub)
        break
      }

      if (t === '+') {
        if (!node.plus) node.plus = new TrieNode(node, { kind: 'plus' })
        node = node.plus
        if (i === sub.tokens.length - 1) {
          node.handlers.set(sub.id, sub)
        }
        continue
      }

      const key = t as string
      let child = node.literals.get(key)
      if (!child) {
        child = new TrieNode(node, { kind: 'literal', key })
        node.literals.set(key, child)
      }
      node = child

      if (i === sub.tokens.length - 1) {
        node.handlers.set(sub.id, sub)
      }
    }
  }

  removeSubscription(sub: InternalSubscription<TPayload, TMeta, TContext>): void {
    let node: TrieNode<TPayload, TMeta, TContext> | undefined = this.root

    for (let i = 0; i < sub.tokens.length; i++) {
      const t = sub.tokens[i]

      if (t === '#') {
        node.hashHandlers.delete(sub.id)
        this.prune(node)
        return
      }

      if (t === '+') {
        node = node.plus
      } else {
        node = node.literals.get(t as string)
      }

      if (!node) return

      if (i === sub.tokens.length - 1) {
        node.handlers.delete(sub.id)
        this.prune(node)
        return
      }
    }
  }

  match(topic: string): InternalSubscription<TPayload, TMeta, TContext>[] {
    const levels = splitTopic(topic)
    const result = new Map<string, InternalSubscription<TPayload, TMeta, TContext>>()

    const walk = (node: TrieNode<TPayload, TMeta, TContext>, idx: number): void => {
      for (const [id, sub] of node.hashHandlers) result.set(id, sub)

      if (idx === levels.length) {
        for (const [id, sub] of node.handlers) result.set(id, sub)
        return
      }

      const lit = node.literals.get(levels[idx]!)
      if (lit) walk(lit, idx + 1)
      if (node.plus) walk(node.plus, idx + 1)
    }

    walk(this.root, 0)
    return Array.from(result.values())
  }

  clear(): void {
    this.root.literals.clear()
    this.root.plus = undefined
    this.root.handlers.clear()
    this.root.hashHandlers.clear()
  }

  private prune(from: TrieNode<TPayload, TMeta, TContext>): void {
    let node: TrieNode<TPayload, TMeta, TContext> | undefined = from
    while (node && node !== this.root) {
      if (!node.isEmpty()) return
      const parent: TrieNode<TPayload, TMeta, TContext> | undefined = node.parent
      if (!parent) return

      const edge = node.edgeFromParent
      if (edge.kind === 'literal') {
        parent.literals.delete(edge.key)
      } else if (edge.kind === 'plus') {
        parent.plus = undefined
      }
      node = parent
    }
  }
}

export function splitTopic(topic: string): string[] {
  return topic.split('/')
}

export function validatePublishTopic(topic: string): void {
  if (topic.length === 0) throw new DynInvalidTopicError(topic, 'Topic must be non-empty')
  if (topic.includes('+') || topic.includes('#')) {
    throw new DynInvalidTopicError(topic, 'Publish topic must not contain wildcards')
  }
}

export function parseFilter(filter: string, validate: boolean): Token[] {
  if (filter.length === 0) throw new DynInvalidFilterError(filter, 'Filter must be non-empty')
  const parts = filter.split('/')
  const tokens: Token[] = parts.map((p) => (p === '+' || p === '#' ? (p as Token) : p))

  if (!validate) return tokens

  for (let i = 0; i < tokens.length; i++) {
    const t = tokens[i]
    if (t === '#' && i !== tokens.length - 1) {
      throw new DynInvalidFilterError(filter, "'#' must be the last level")
    }
    if (t !== '+' && t !== '#') {
      const s = t as string
      if (s.includes('+') || s.includes('#')) {
        throw new DynInvalidFilterError(filter, 'Wildcards must occupy an entire level')
      }
    }
  }

  return tokens
}
