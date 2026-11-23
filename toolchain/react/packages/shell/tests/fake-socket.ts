// A WebSocket double: tests open it, push frames into it and read what the page sent.

export class FakeSocket extends EventTarget {
  static all: FakeSocket[] = [];
  readonly sent: unknown[] = [];
  closed = false;

  constructor(readonly url: string) {
    super();
    FakeSocket.all.push(this);
  }

  static reset(): void {
    FakeSocket.all = [];
  }

  static last(): FakeSocket {
    const socket = FakeSocket.all.at(-1);
    if (!socket) throw new Error("no socket was opened");
    return socket;
  }

  send(text: string): void {
    this.sent.push(JSON.parse(text));
  }

  close(): void {
    if (this.closed) return;
    this.closed = true;
    this.dispatchEvent(new Event("close"));
  }

  open(): void {
    this.dispatchEvent(new Event("open"));
  }

  frame(value: unknown): void {
    this.dispatchEvent(new MessageEvent("message", { data: JSON.stringify(value) }));
  }
}

export const fakeWebSocket = FakeSocket as unknown as new (url: string) => WebSocket;

export const tick = (ms = 0) => new Promise((resolve) => setTimeout(resolve, ms));
