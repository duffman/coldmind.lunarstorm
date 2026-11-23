export type Comparator<T> = (a: T, b: T) => number

export class PriorityHeap<T> {
  private heap: T[] = []

  constructor(private readonly comparator: Comparator<T>) {}

  size(): number {
    return this.heap.length
  }

  isEmpty(): boolean {
    return this.heap.length === 0
  }

  peek(): T | undefined {
    return this.heap[0]
  }

  push(item: T): void {
    this.heap.push(item)
    this.bubbleUp(this.heap.length - 1)
  }

  pushMany(items: T[]): void {
    for (const item of items) {
      this.push(item)
    }
  }

  pop(): T | undefined {
    if (this.heap.length === 0) return undefined
    if (this.heap.length === 1) return this.heap.pop()

    const root = this.heap[0]
    this.heap[0] = this.heap.pop()!
    this.bubbleDown(0)
    return root
  }

  popMany(count: number): T[] {
    const result: T[] = []
    const actualCount = Math.min(count, this.heap.length)
    for (let i = 0; i < actualCount; i++) {
      const item = this.pop()
      if (item !== undefined) result.push(item)
    }
    return result
  }

  clear(): void {
    this.heap.length = 0
  }

  private bubbleUp(index: number): void {
    while (index > 0) {
      const parentIndex = Math.floor((index - 1) / 2)
      if (this.comparator(this.heap[parentIndex]!, this.heap[index]!) >= 0) break
      this.swap(index, parentIndex)
      index = parentIndex
    }
  }

  private bubbleDown(index: number): void {
    const lastIndex = this.heap.length - 1
    while (true) {
      const leftChild = 2 * index + 1
      const rightChild = 2 * index + 2
      let highestPriority = index

      if (
        leftChild <= lastIndex &&
        this.comparator(this.heap[leftChild]!, this.heap[highestPriority]!) > 0
      ) {
        highestPriority = leftChild
      }
      if (
        rightChild <= lastIndex &&
        this.comparator(this.heap[rightChild]!, this.heap[highestPriority]!) > 0
      ) {
        highestPriority = rightChild
      }
      if (highestPriority === index) break

      this.swap(index, highestPriority)
      index = highestPriority
    }
  }

  private swap(i: number, j: number): void {
    const temp = this.heap[i]
    this.heap[i] = this.heap[j]!
    this.heap[j] = temp!
  }
}
