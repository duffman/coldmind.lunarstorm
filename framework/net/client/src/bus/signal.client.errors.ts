import {
  DynNoResponderError, DynRateLimitExceededError, DynRequestCancelledError, DynRequestError, DynRequestTimeoutError,
} from "@coldmind/bus";
import { RpcError } from "@coldmind/rpc";

/** Protocol errors as the bus errors a local bus would throw: what a RemoteBus throws, so code written against a local bus sees the errors it expects. */
export function toBusError(e: unknown, topic: string, correlationId: string, timeoutMs: number): unknown {
  if (!(e instanceof RpcError)) return e;
  switch (e.code) {
    case "NO_RESPONDER":
      return new DynNoResponderError(topic);
    case "TIMEOUT":
      return new DynRequestTimeoutError(timeoutMs, topic, correlationId);
    case "CANCELLED":
      return new DynRequestCancelledError(topic, correlationId);
    case "BUSY":
      return new DynRateLimitExceededError("request");
    default:
      return new DynRequestError(e.code, e.message, e.data, topic, correlationId);
  }
}

