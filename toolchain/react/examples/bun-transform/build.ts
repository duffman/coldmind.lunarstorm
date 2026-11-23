import { join } from "node:path";
import { devtools } from "@coldmind/devtools";

const plugin = devtools({
  define: { __MODE__: JSON.stringify("example") },
  inject: true,
});
const result = await Bun.build({ entrypoints: [join(import.meta.dir, "app/entry.ts")], plugins: [plugin] });
if (!result.success) throw new AggregateError(result.logs, "build failed");

console.log(await result.outputs[0]!.text());
console.log(plugin.stats());
