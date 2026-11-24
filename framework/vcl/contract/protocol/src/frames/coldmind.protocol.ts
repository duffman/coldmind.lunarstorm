import type { Model } from "@coldmind/rpc";

/** The protocol. Every external protocol is translated to and from these frames. See PROTOCOL.md. */
export const PROTOCOL_VERSION = 1 as const;

/** Chosen by the caller: a safe integer, or a string of 1–128 characters. */
export type FrameId = number | string;
/** Credentials carried by a call. The protocol adapter fills it; an auth adapter verifies it. */
export type AuthSlot = { method: string; credentials: Model };

/** Call `name` (`controller.method`) with `args`. RPC and bus traffic alike are calls. */
export type CallFrame = {
  v: 1;
  kind: "call";
  id: FrameId;
  name: string;
  args: unknown;
  /** Session reference. */
  sid?: string;
  /** Per-call credentials: authenticates this call only, no session involved. */
  auth?: AuthSlot;
  meta?: Model;
  /** Deadline for the whole call, in milliseconds. The server stops the call after the lower of this and its own limit. */
  timeoutMs?: number;
};
/** Stop the call with that id. Never answered itself. */
export type CancelFrame = { v: 1; kind: "cancel"; id: FrameId };

/**
 * What carries "the session ended, drop your id" in a header-based transport. An empty header value cannot be used:
 * servers drop it on the wire. Session ids are 43 characters, so a single dash never collides with one.
 * @example
 * response.headers.set("x-session-id", SID_CLEARED);
 */
export const SID_CLEARED = "-";

/** The call ended with `value`. `sid`: a new session id, or null when the session ended. */
export type ResultFrame = { v: 1; kind: "result"; id: FrameId; value: unknown; sid?: string | null };
/** The call failed with an `RpcError` code. `id` is null when the frame itself could not be read. */
export type ErrorFrame = { v: 1; kind: "error"; id: FrameId | null; code: string; message: string; data?: unknown; sid?: string | null };
/** One streamed part of a call still running (a subscription's messages, a proc's `chunk`s). */
export type ChunkFrame = { v: 1; kind: "chunk"; id: FrameId; value: unknown };
/** Server-initiated, not tied to a call: `server.publish(def, data, to)`. */
export type EventFrame = { v: 1; kind: "event"; name: string; data: unknown };

/** Frames a server is fed. */
export type InFrame = CallFrame | CancelFrame;
/** Frames a server sends. */
export type OutFrame = ResultFrame | ErrorFrame | ChunkFrame | EventFrame;
/** Every frame of the protocol; what a `Codec` encodes. */
export type Frame = InFrame | OutFrame;

type Sid = string | null | undefined;
const withSid = (sid: Sid) => (sid !== undefined ? { sid } : {});

/** Frame constructors; optional fields are left out rather than set to undefined. */
export const frames = {
  call: (id: FrameId, name: string, args: unknown, extra: Pick<CallFrame, "sid" | "auth" | "meta" | "timeoutMs"> = {}): CallFrame => ({
    v: 1, kind: "call", id, name, args,
    ...(extra.sid !== undefined ? { sid: extra.sid } : {}),
    ...(extra.auth ? { auth: extra.auth } : {}),
    ...(extra.meta ? { meta: extra.meta } : {}),
    ...(extra.timeoutMs !== undefined ? { timeoutMs: extra.timeoutMs } : {}),
  }),
  cancel: (id: FrameId): CancelFrame => ({ v: 1, kind: "cancel", id }),
  result: (id: FrameId, value: unknown, sid?: Sid): ResultFrame => ({ v: 1, kind: "result", id, value, ...withSid(sid) }),
  error: (id: FrameId | null, code: string, message: string, data?: unknown, sid?: Sid): ErrorFrame => ({
    v: 1, kind: "error", id, code, message, ...(data !== undefined ? { data } : {}), ...withSid(sid),
  }),
  chunk: (id: FrameId, value: unknown): ChunkFrame => ({ v: 1, kind: "chunk", id, value }),
  event: (name: string, data: unknown): EventFrame => ({ v: 1, kind: "event", name, data }),
};

/** A plain object: not null, not an array. */
export const isObj = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null && !Array.isArray(v);
const isId = (v: unknown): v is FrameId =>
  typeof v === "string" ? v.length > 0 && v.length <= 128 : typeof v === "number" && Number.isSafeInteger(v);
const isSidOut = (v: unknown): v is Sid => v === undefined || v === null || typeof v === "string";

/** Validates what a server is fed. On failure, `reject` is the error frame to send back. */
export function parseInFrame(raw: unknown): { frame: InFrame } | { reject: ErrorFrame } {
  const bad = (id: FrameId | null, message: string, code = "BAD_FRAME") => ({ reject: frames.error(id, code, message) });
  if (!isObj(raw)) return bad(null, "Frame must be an object");
  const id = isId(raw.id) ? raw.id : null;
  if (raw.v !== PROTOCOL_VERSION) return bad(id, `Protocol version ${PROTOCOL_VERSION} required`, "UNSUPPORTED_VERSION");
  if (id === null) return bad(null, "Missing or invalid id");
  if (raw.kind === "cancel") return { frame: frames.cancel(id) };
  if (raw.kind !== "call") return bad(id, "Unknown frame kind");
  if (typeof raw.name !== "string") return bad(id, "name must be a string");
  if (raw.sid !== undefined && typeof raw.sid !== "string") return bad(id, "sid must be a string");
  if (raw.meta !== undefined && !isObj(raw.meta)) return bad(id, "meta must be an object");
  const timeoutMs = raw.timeoutMs;
  if (timeoutMs !== undefined && !(typeof timeoutMs === "number" && Number.isSafeInteger(timeoutMs) && timeoutMs >= 1 && timeoutMs <= 2_147_483_647))
    return bad(id, "timeoutMs must be a positive integer");
  const auth = raw.auth;
  if (auth !== undefined && !(isObj(auth) && typeof auth.method === "string" && isObj(auth.credentials)))
    return bad(id, "auth must be { method, credentials }");
  return {
    frame: frames.call(id, raw.name, raw.args, {
      sid: raw.sid,
      auth: auth as AuthSlot | undefined,
      meta: raw.meta,
      timeoutMs,
    }),
  };
}

/** Validates what a client receives. Null for anything that is not a frame a server sends. */
export function parseOutFrame(raw: unknown): OutFrame | null {
  if (!isObj(raw) || raw.v !== PROTOCOL_VERSION) return null;
  if (raw.kind === "event") return typeof raw.name === "string" ? frames.event(raw.name, raw.data) : null;
  if (raw.kind === "error") {
    if (!(raw.id === null || isId(raw.id)) || typeof raw.code !== "string" || typeof raw.message !== "string" || !isSidOut(raw.sid)) return null;
    return frames.error(raw.id, raw.code, raw.message, raw.data, raw.sid);
  }
  if (!isId(raw.id)) return null;
  if (raw.kind === "chunk") return frames.chunk(raw.id, raw.value);
  if (raw.kind === "result") return isSidOut(raw.sid) ? frames.result(raw.id, raw.value, raw.sid) : null;
  return null;
}
