// Calls every layer's declarations the way a host module does, through the `ops`
// object of `lunarstorm:internal` (the only declared module until the node:*
// modules are ported). Compiled by `pnpm --filter lunartypes typecheck`, never run.
import { ops, type Ops } from "lunarstorm:internal";

// Core: index.d.ts.
const argv: string[] = ops.argv;
const cwd: string = ops.cwd();
const bytes: Uint8Array = ops.utf8Encode(`${ops.platform} ${ops.arch} ${ops.pid} ${argv.length} ${cwd}`);
ops.print(1, ops.utf8Decode(bytes));
const [key = 0, port = 0] = ops.listen("127.0.0.1", 0, false);
const timer: number = ops.timerStart(port, false);
const active: boolean = ops.timerCancel(timer);
const queued: boolean = ops.write(ops.connect("localhost", 80), active ? bytes : null);
ops.httpRespond(key, 200, "content-type: text/plain\r\n", undefined, queued);
const read: unknown = ops.fs(0, "data.txt", undefined, 0);
ops.settled(1, true, read);

// HostApi's `ui` area: extensions/hostapi.d.ts.
const width: number = ops.columns();
ops.live(width > 0);

// The desktop host: desktop.d.ts.
ops.desktopHooks(
  (json) => JSON.parse(json),
  (id, ok, json) => ops.desktopError(`${id} ${ok} ${json}`),
);
ops.desktopNative(1, "dialog", "open", ops.desktopRestore());

export const members: (keyof Ops)[] = ["argv", "columns", "desktopSend"];
