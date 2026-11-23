import { RpcError } from "@coldmind/rpc";
import type { InFrame } from "@coldmind/protocol";

/**
 * Client-side adapter slot: takes frames of the protocol and gets them to a server, natively or by
 * translating to another protocol. Delivers what comes back as decoded structure; the Client validates it.
 */
export type Link = {
  send(frame: InFrame): void | Promise<void>;
  onFrame(cb: (frame: unknown) => void): void;
  /** Fires once the link can carry frames; at once if it already can. */
  onOpen(cb: () => void): void;
  onClose(cb: () => void): void;
  close(): void;
  /**
   * Only on links that survive a lost connection: reports "down" when the connection is lost and
   * "up" when it is back. A link without it is simply closed when its connection ends.
   */
  onState?(cb: (state: "up" | "down") => void): void;
};

/**
 * Building block for links: callback wiring, close-once semantics, send-after-close.
 * Pass `open: false` for a link that has to connect first, and call `opened()` when it has.
 */
export function linkEndpoint(send: (frame: InFrame) => void | Promise<void>, requestClose: () => void, init: { open?: boolean } = {}) {
  const frameCbs: ((frame: unknown) => void)[] = [];
  const closeCbs: (() => void)[] = [];
  const openCbs: (() => void)[] = [];
  let closed = false;
  let open = init.open ?? true;
  const markClosed = () => {
    if (closed) return;
    closed = true;
    for (const cb of closeCbs.splice(0)) cb();
  };
  const link: Link = {
    send(frame) {
      if (closed) throw new RpcError("CLOSED", "Connection closed");
      return send(frame);
    },
    onFrame: cb => void frameCbs.push(cb),
    onOpen: cb => (open ? cb() : void openCbs.push(cb)),
    onClose: cb => (closed ? cb() : void closeCbs.push(cb)),
    close() {
      if (closed) return;
      requestClose();
      markClosed();
    },
  };
  return {
    link,
    emit(frame: unknown) {
      if (!closed) for (const cb of frameCbs) cb(frame);
    },
    closed: markClosed,
    opened() {
      if (open || closed) return;
      open = true;
      for (const cb of openCbs.splice(0)) cb();
    },
  };
}
