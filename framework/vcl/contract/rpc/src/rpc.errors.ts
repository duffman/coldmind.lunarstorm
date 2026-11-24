import type { ValidationError } from "@coldmind/core";
import { ValidationFailed, validate, validateAsync, validateLocal, type OutputOf, type Schema } from "@coldmind/rpc/src/rpc.model.ts";

/**
 * Neutral error set of the protocol. Server side: BAD_FRAME, UNSUPPORTED_VERSION, UNKNOWN_PROC, INVALID_ARGS,
 * UNAUTHENTICATED, FORBIDDEN, AUTH_FAILED, CANCELLED, TIMEOUT, BUSY, INTERNAL. Client side adds CLOSED,
 * TRANSPORT, INVALID_RESULT. Handlers may throw RpcError with codes of their own.
 */
export class RpcError extends Error {
  constructor(readonly code: string, message: string, readonly data?: unknown) {
    super(message);
    this.name = "RpcError";
  }
}

/** What `data` holds on INVALID_ARGS and INVALID_RESULT: the model layer's errors, unchanged. */
export type ValidationData = { errors: ValidationError[] };

/** The validation errors an RpcError carries, or an empty list. */
export function validationErrors(error: unknown): ValidationError[] {
  const data = error instanceof RpcError ? (error.data as Partial<ValidationData> | undefined) : undefined;
  return Array.isArray(data?.errors) ? data.errors : [];
}

const asRpc = (e: unknown, code: string): RpcError =>
  e instanceof ValidationFailed
    ? new RpcError(code, e.message, { errors: [...e.errors] } satisfies ValidationData)
    : new RpcError(code, e instanceof Error ? e.message : String(e));

/** Validates; a failure becomes an RpcError with `code` and the validation errors in `data.errors`. */
export function parseWith<S extends Schema>(schema: S, input: unknown, code: string): OutputOf<S> {
  try {
    return validate(schema, input);
  } catch (e) {
    throw asRpc(e, code);
  }
}

/** `parseWith` including async validators. For the side that owns the data. */
export async function parseWithAsync<S extends Schema>(schema: S, input: unknown, code: string): Promise<OutputOf<S>> {
  try {
    return await validateAsync(schema, input);
  } catch (e) {
    throw asRpc(e, code);
  }
}

/** `parseWith` for the side that does not own the data: schemas with async validators pass through. */
export function parseLocal<S extends Schema>(schema: S, input: unknown, code: string): OutputOf<S> {
  try {
    return validateLocal(schema, input);
  } catch (e) {
    throw asRpc(e, code);
  }
}
