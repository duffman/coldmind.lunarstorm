// Calls the types must reject: each line after `@ts-expect-error` is an error, and
// `tsc` fails if one is not (plan AC6).
import { ops } from "lunarstorm:internal";
import { progress, spinner } from "lunarstorm:hostapi/ui";
import { setProperty } from "@desktop/bridge";

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
// @ts-expect-error: progress's total is a number
progress({ total: "5" });
// @ts-expect-error: a spinner has no update
spinner().update(1);
// @ts-expect-error: setProperty's control is a string
setProperty("Main", 1, "text", "x");
