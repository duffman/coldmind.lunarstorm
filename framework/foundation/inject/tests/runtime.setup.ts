/** The runtime a functional or coverage command requires inside its test worker. */
const expectedRuntime = process.env.CMINJECT_EXPECT_RUNTIME;
/** Bun identifies itself even though its process object also exposes a Node compatibility version. */
const actualRuntime = "bun" in process.versions ? "bun" : "node";
if (expectedRuntime && expectedRuntime !== actualRuntime) throw new Error(`Expected ${expectedRuntime} test worker, got ${actualRuntime}`);
