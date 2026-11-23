import type { DynStream } from '@coldmind/bus/src/types.ts'

/**
 * Turns a request that reports chunks through a callback into an async iterable.
 * `start` receives the chunk callback and an abort signal and returns the final result.
 * Leaving the loop early cancels the request.
 */
export function createStream<TChunk, TRes>(
  start: (onChunk: (chunk: unknown) => void, signal: AbortSignal) => Promise<TRes>,
): DynStream<TChunk, TRes> {
  const controller = new AbortController()
  const buffer: TChunk[] = []
  let waiter: (() => void) | undefined
  let done = false
  let failure: { error: unknown } | undefined

  const wake = (): void => {
    const w = waiter
    waiter = undefined
    w?.()
  }

  const result = start((chunk) => {
    buffer.push(chunk as TChunk)
    wake()
  }, controller.signal)

  result.then(
    () => {
      done = true
      wake()
    },
    (error) => {
      failure = { error }
      wake()
    },
  )

  return {
    result,
    cancel: () => controller.abort(),
    async *[Symbol.asyncIterator]() {
      try {
        for (;;) {
          if (buffer.length > 0) {
            yield buffer.shift() as TChunk
            continue
          }
          if (failure) throw failure.error
          if (done) return
          await new Promise<void>((resolve) => {
            waiter = resolve
          })
        }
      } finally {
        if (!done && !failure) controller.abort()
      }
    },
  }
}
