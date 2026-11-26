/**
 * Minimal CLI host demo: reads a file through node:fs and fires a timer.
 *
 * Usage: _build/lunarstorm platform/cli/host/examples/demo.js <file>
 * @param process.argv[2] path of the file whose line count is printed
 */
import { readFileSync } from "node:fs";
console.log("lines:", readFileSync(process.argv[2], "utf8").split("\n").length);
setTimeout(() => console.log("timer fired"), 10);
