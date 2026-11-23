/** @module @coldmind/devermind-shell/protocol — frames on the devermind socket (Vite's HMR protocol). */

/** One `js-update` / `css-update` entry of an `update` frame. */
export interface HmrUpdate {
  type: "js-update" | "css-update";
  /** URL of the boundary module that receives the update. */
  path: string;
  /** URL of the module that is re-imported; the boundary itself when it self-accepts. */
  acceptedPath?: string;
  /** Server stamp, appended as `?t=` so the browser fetches the new version. */
  timestamp: number;
  isWithinCircularImport?: boolean;
  firstInvalidatedBy?: string;
}

/** A compile or server error as the server reports it. */
export interface ServerErrorPayload {
  message: string;
  stack?: string;
  id?: string;
  frame?: string;
  plugin?: string;
  loc?: { file?: string; line?: number; column?: number };
}

/** Every frame the server sends. `custom` frames carry HMR plugin events and app channel messages. */
export type ServerFrame =
  | { type: "connected" }
  | { type: "update"; updates: HmrUpdate[] }
  | { type: "full-reload"; path?: string; reason?: string }
  | { type: "prune"; paths: string[] }
  | { type: "error"; err?: ServerErrorPayload; message?: string }
  | { type: "custom"; event: string; data?: unknown };

/** Event prefix of app channel messages: channel `chat` travels as event `app:chat`. */
export const APP_EVENT_PREFIX = "app:";

/** Path of the devermind HMR socket (`HMR_PATH` in the server). */
export const HMR_PATH = "/__devermind_hmr";

/**
 * Parses one socket message into a frame.
 * @returns The frame, or `null` for anything that is not a JSON object with a string `type`.
 */
export function parseFrame(text: unknown): ServerFrame | null {
  if (typeof text !== "string") return null;
  try {
    const value: unknown = JSON.parse(text);
    if (value && typeof value === "object" && typeof (value as { type?: unknown }).type === "string") {
      return value as ServerFrame;
    }
  } catch {}
  return null;
}
