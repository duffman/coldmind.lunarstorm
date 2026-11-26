// Calls every layer's declarations the way a host module does, through the `ops`
// object of `lunarstorm:internal` (the only declared module until the node:*
// modules are ported). Compiled by `pnpm --filter lunartypes typecheck`, never run.
import { ops, type Ops } from "lunarstorm:internal";
import { areas } from "lunarstorm:hostapi";
import { progress, spinner, style } from "lunarstorm:hostapi/ui";
import { restored, serve, setProperty } from "@desktop/bridge";
import { native } from "@desktop/api";

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

// HostApi and its `ui` area: extensions/hostapi.d.ts.
const bar = progress({ total: areas.length, label: style.bold("areas"), bytes: false });
bar.tick();
bar.update(bar.value + 1);
spinner(style.rgb(1, 2, 3)("wait")).succeed(style.gradient("done"));

// The desktop host: desktop.d.ts.
serve((message) => (message.request ? restored(message.form) : setProperty(message.form, message.target, "text", message.payload)));
export const opened: unknown = native.dialog.open(ops.cwd());

export const members: (keyof Ops)[] = ["argv", "print", "now"];
