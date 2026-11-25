import type { DynMessage, Token } from '@coldmind/bus/src/types.ts'
import { splitTopic } from '@coldmind/bus/src/internal/matcher.ts'

class RetainedTrieNode<TPayload, TMeta> {
  children = new Map<string, RetainedTrieNode<TPayload, TMeta>>()
  message?: DynMessage<TPayload, TMeta>
}

export class RetainedStore<TPayload, TMeta> {
  private readonly root = new RetainedTrieNode<TPayload, TMeta>()
  private count = 0

  set(msg: DynMessage<TPayload, TMeta>): void {
    const levels = splitTopic(msg.topic)
    if (msg.payload === undefined || msg.payload === null) {
      this.removeFromTrie(levels)
    } else {
      this.addToTrie(levels, msg)
    }
  }

  match(tokens: Token[]): DynMessage<TPayload, TMeta>[] {
    const result: DynMessage<TPayload, TMeta>[] = []
    this.matchRecursive(this.root, tokens, 0, result)
    return result
  }

  get(topic: string): DynMessage<TPayload, TMeta> | undefined {
    let node = this.root
    for (const level of splitTopic(topic)) {
      const child = node.children.get(level)
      if (!child) return undefined
      node = child
    }
    return node.message
  }

  clear(): void {
    this.root.children.clear()
    this.root.message = undefined
    this.count = 0
  }

  get size(): number {
    return this.count
  }

  private addToTrie(levels: string[], msg: DynMessage<TPayload, TMeta>): void {
    let node = this.root
    for (const level of levels) {
      let child = node.children.get(level)
      if (!child) {
        child = new RetainedTrieNode<TPayload, TMeta>()
        node.children.set(level, child)
      }
      node = child
    }
    if (!node.message) this.count++
    node.message = msg
  }

  private removeFromTrie(levels: string[]): void {
    const path: Array<{ node: RetainedTrieNode<TPayload, TMeta>; level: string }> = []
    let node = this.root

    for (const level of levels) {
      const child = node.children.get(level)
      if (!child) return
      path.push({ node, level })
      node = child
    }

    if (node.message) {
      node.message = undefined
      this.count--
    }

    for (let i = path.length - 1; i >= 0; i--) {
      const { node: parentNode, level } = path[i]!
      const childNode = parentNode.children.get(level)
      if (childNode && childNode.children.size === 0 && !childNode.message) {
        parentNode.children.delete(level)
      } else {
        break
      }
    }
  }

  private matchRecursive(
    node: RetainedTrieNode<TPayload, TMeta>,
    tokens: Token[],
    tokenIndex: number,
    result: DynMessage<TPayload, TMeta>[],
  ): void {
    if (tokenIndex === tokens.length) {
      if (node.message) result.push(node.message)
      return
    }

    const token = tokens[tokenIndex]

    if (token === '#') {
      this.collectAllMessages(node, result)
      return
    }

    if (token === '+') {
      for (const child of node.children.values()) {
        this.matchRecursive(child, tokens, tokenIndex + 1, result)
      }
      return
    }

    const child = node.children.get(token as string)
    if (child) {
      this.matchRecursive(child, tokens, tokenIndex + 1, result)
    }
  }

  private collectAllMessages(
    node: RetainedTrieNode<TPayload, TMeta>,
    result: DynMessage<TPayload, TMeta>[],
  ): void {
    if (node.message) result.push(node.message)
    for (const child of node.children.values()) {
      this.collectAllMessages(child, result)
    }
  }
}
