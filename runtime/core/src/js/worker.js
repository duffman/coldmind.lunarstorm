// `lunarstorm:worker`: a part of the program on its own thread, in its own VM.
//
//   import { spawn, postMessage, onMessage, close } from 'lunarstorm:worker';
//
// This module is the JS half. The VM contract has no way to build a promise from
// native code, so the handle class and its `exited` promise live here and call the
// natives of `lunarstorm:worker-native`, which only this module may import.
//
// Message format (decision): a message crosses threads as bytes. The first byte is
// a tag: 0 = the rest is UTF-8 JSON text, 1 = the rest is a `Uint8Array`'s bytes.
// So structured data means JSON values and a top-level `Uint8Array`; functions,
// symbols and `undefined` are refused with a TypeError, and a nested `Uint8Array`
// arrives as a JSON object. Dropped alternative: a full structured clone, which the
// VM contract does not offer.
import * as native from 'lunarstorm:worker-native';

const encoder = new TextEncoder();
const decoder = new TextDecoder();

/**
 * Turns a message into the bytes that cross threads.
 * @param {unknown} data JSON-compatible data or a Uint8Array
 * @return {Uint8Array} the tag byte followed by the payload
 */
function encode(data) {
  if (data instanceof Uint8Array) {
    const out = new Uint8Array(data.length + 1);
    out[0] = 1;
    out.set(data, 1);
    return out;
  }
  const json = JSON.stringify(data);
  if (json === undefined) throw new TypeError('postMessage expects structured data');
  const text = encoder.encode(json);
  const out = new Uint8Array(text.length + 1);
  out[0] = 0;
  out.set(text, 1);
  return out;
}

/**
 * Turns the bytes of a message back into data.
 * @param {Uint8Array} bytes the tag byte followed by the payload
 * @return {unknown} the data
 */
function decode(bytes) {
  return bytes[0] === 1 ? bytes.slice(1) : JSON.parse(decoder.decode(bytes.subarray(1)));
}

/**
 * Wraps a listener so it receives decoded data.
 * @param {Function|null} listener the user's listener
 * @return {Function|null} what to hand to the native side
 */
function wrap(listener) {
  if (listener === null || listener === undefined) return null;
  if (typeof listener !== 'function') throw new TypeError('onMessage expects a function or null');
  return (bytes) => listener(decode(bytes));
}

/** A worker as its parent sees it. */
class Worker {
  /**
   * @param {number} id the worker's id, unique in the parent
   */
  constructor(id) {
    /** Positive integer, unique within the parent VM. */
    this.id = id;
    /** Settles once: `{ state: 'fulfilled' | 'terminated' }`, or rejects with a WorkerError. */
    this.exited = new Promise((resolve, reject) => {
      native.onExit(id, (state, message) => {
        if (state === 'failed') {
          const error = new Error(message);
          error.code = 'WorkerError';
          reject(error);
        } else {
          resolve({ state });
        }
      });
    });
  }

  /**
   * Sends data to the worker.
   * @param {unknown} data JSON-compatible data or a Uint8Array
   * @return {boolean} true when the worker will receive it; false when it was refused
   *   (the worker is gone or closing, its mailbox is full, or the message is over 8 MiB)
   */
  postMessage(data) {
    return native.post(this.id, encode(data));
  }

  /**
   * Receives the worker's messages in order.
   * @param {Function|null} listener called with each message's data; null removes it
   */
  onMessage(listener) {
    native.listen(this.id, wrap(listener));
  }

  /** Asks the worker to stop; does not wait. Its own workers end with it. */
  terminate() {
    native.terminate(this.id);
  }
}

/**
 * Starts a worker on a new thread.
 * @param {string} path the worker's entry module: absolute, or relative to the
 *   directory of the program's entry module
 * @param {{ maxMemory?: number }} [options] `maxMemory` is the heap ceiling in bytes
 * @return {Worker} the handle
 */
export function spawn(path, options = {}) {
  if (typeof path !== 'string') throw new TypeError('spawn expects a path');
  const maxMemory = options.maxMemory === undefined ? 268435456 : options.maxMemory;
  if (!Number.isInteger(maxMemory) || maxMemory <= 0) {
    throw new RangeError('maxMemory must be a positive byte count');
  }
  return new Worker(native.spawn(path, maxMemory));
}

/**
 * Inside a worker: sends data to the parent.
 * @param {unknown} data JSON-compatible data or a Uint8Array
 * @return {boolean} true when the parent will receive it; false outside a worker or when refused
 */
export function postMessage(data) {
  const bytes = encode(data);
  return native.isWorker() ? native.post(0, bytes) : false;
}

/**
 * Inside a worker: receives the parent's messages. A set listener keeps the worker
 * alive until `close()` or `terminate()`; null releases it.
 * @param {Function|null} listener called with each message's data
 * @return {boolean} true inside a worker, false outside
 */
export function onMessage(listener) {
  const wrapped = wrap(listener);
  if (!native.isWorker()) return false;
  native.listenParent(wrapped);
  return true;
}

/** Inside a worker: stops accepting messages and ends the worker, even with timers pending. */
export function close() {
  if (native.isWorker()) native.close();
}
