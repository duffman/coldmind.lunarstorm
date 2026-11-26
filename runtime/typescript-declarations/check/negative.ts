// Calls the types must reject: each line after `@ts-expect-error` is an error, and
// `tsc` fails if one is not (plan AC6).
import { ops } from "lunarstorm:internal";

// @ts-expect-error: fd is a number
ops.print("1", "text");
// @ts-expect-error: utf8Decode takes bytes
ops.utf8Decode("text");
// @ts-expect-error: timerStart needs `repeat`
ops.timerStart(10);
// @ts-expect-error: data is a string, bytes, null or undefined
ops.write(1, 42);
// @ts-expect-error: no such op
ops.missing();
// @ts-expect-error: columns returns a number
export const width: string = ops.columns();
// @ts-expect-error: live takes a boolean
ops.live("on");
// @ts-expect-error: settle's `ok` is a boolean
ops.desktopHooks((json: string) => json, (id: number, ok: string) => id + ok);
